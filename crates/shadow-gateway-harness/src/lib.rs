#![forbid(unsafe_code)]

//! Credentialless deterministic shadow gateway around the Phase 2.14 runtime.
//!
//! The harness consumes recorded fixtures and simulated observations only. It
//! cannot load credentials, sign, authenticate, call a network or RPC endpoint,
//! access a wallet or relayer, or submit an order or transaction.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableShadowGateway,
    GatewayCheckpoint, GatewayRecovery, StorageError,
};

use order_intent_policy::{ExchangeMode, ExchangeModeObservation};
use serde::{Deserialize, Serialize};
use shadow_adapter_certification::{
    CertificationReport, CertificationStatus, FixtureId, FixtureKind, RecordedFixture,
};
use std::collections::BTreeMap;
use thiserror::Error;
use unified_paired_trading_runtime::{
    UnifiedCommand, UnifiedCommandId, UnifiedOutcome, UnifiedPairedTradingRuntime,
};

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct GatewayCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    pub expected_contract_digest: [u8; 32],
    pub certification_max_age_ns: i64,
    pub heartbeat_max_age_ns: i64,
    pub mode_validity_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct StackHeartbeat {
    pub sequence: u64,
    pub strategy_healthy: bool,
    pub risk_healthy: bool,
    pub market_feed_healthy: bool,
    pub user_feed_healthy: bool,
    pub ledger_reconciled: bool,
    pub observed_at_ns: i64,
    pub valid_until_ns: i64,
    pub observation_digest: [u8; 32],
}

impl StackHeartbeat {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.observation_digest = heartbeat_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.observation_digest == heartbeat_digest(self)
    }

    #[must_use]
    pub const fn completely_healthy(&self) -> bool {
        self.strategy_healthy
            && self.risk_healthy
            && self.market_feed_healthy
            && self.user_feed_healthy
            && self.ledger_reconciled
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayMode {
    Starting,
    CertificationRequired,
    CertificationExpired,
    Ready,
    PostOnly,
    CancelOnly,
    DelayActive,
    Backoff,
    BackingRetained,
    Restarting,
    Recovering,
    DeadManTriggered,
    Halted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureTranslation {
    EngineRestarting,
    PostOnlyWindow,
    CancelOnlyWindow,
    TakerDelayActive,
    RevalidatePricesAndOrders,
    RateLimitedBackoff,
    UnknownOrderReconciliation,
    SettlementValueRetained,
    DeadManCancelAndDisable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayDenial {
    CertificationMissing,
    CertificationNotCertified,
    CertificationMismatch,
    CertificationExpired,
    HeartbeatMissing,
    HeartbeatUnhealthy,
    HeartbeatExpired,
    GatewayMode,
    CallerModeObservationForbidden,
    RecoveryEvidenceMissing,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum GatewayCommand {
    InstallCertification {
        command_id: GatewayCommandId,
        report: CertificationReport,
        recorded_at_ns: i64,
    },
    ObserveHeartbeat {
        command_id: GatewayCommandId,
        heartbeat: StackHeartbeat,
        recorded_at_ns: i64,
    },
    ApplyFixture {
        command_id: GatewayCommandId,
        fixture: RecordedFixture,
        recorded_at_ns: i64,
    },
    ApplyRuntime {
        command_id: GatewayCommandId,
        command: Box<UnifiedCommand>,
        recorded_at_ns: i64,
    },
    Recover {
        command_id: GatewayCommandId,
        recovery_epoch: u64,
        reconciliation_current: bool,
        unknown_orders_cleared: bool,
        recovery_evidence_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    Tick {
        command_id: GatewayCommandId,
        now_ns: i64,
        recorded_at_ns: i64,
    },
}

impl GatewayCommand {
    #[must_use]
    pub const fn command_id(&self) -> GatewayCommandId {
        match self {
            Self::InstallCertification { command_id, .. }
            | Self::ObserveHeartbeat { command_id, .. }
            | Self::ApplyFixture { command_id, .. }
            | Self::ApplyRuntime { command_id, .. }
            | Self::Recover { command_id, .. }
            | Self::Tick { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::InstallCertification { recorded_at_ns, .. }
            | Self::ObserveHeartbeat { recorded_at_ns, .. }
            | Self::ApplyFixture { recorded_at_ns, .. }
            | Self::ApplyRuntime { recorded_at_ns, .. }
            | Self::Recover { recorded_at_ns, .. }
            | Self::Tick { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum GatewayDetail {
    CertificationInstalled {
        placement_eligible: bool,
        derived: Vec<UnifiedOutcome>,
    },
    HeartbeatObserved {
        dead_man_triggered: bool,
        derived: Vec<UnifiedOutcome>,
    },
    FixtureApplied {
        translation: FixtureTranslation,
        automatic_retry: bool,
        backing_released: bool,
        derived: Vec<UnifiedOutcome>,
    },
    RuntimeApplied(UnifiedOutcome),
    RuntimeDenied {
        reason: GatewayDenial,
        derived: Vec<UnifiedOutcome>,
    },
    Recovered {
        recovery_epoch: u64,
        derived: Vec<UnifiedOutcome>,
    },
    RecoveryDenied(GatewayDenial),
    TickApplied {
        dead_man_triggered: bool,
        certification_expired: bool,
        derived: Vec<UnifiedOutcome>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayOutcome {
    pub command_id: GatewayCommandId,
    pub detail: GatewayDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct GatewaySnapshot {
    pub accepted_commands: u64,
    pub mode: GatewayMode,
    pub certification_report_digest: Option<[u8; 32]>,
    pub certification_fresh: bool,
    pub heartbeat_healthy: bool,
    pub fixture_count: usize,
    pub recovery_epoch: u64,
    pub mode_sequence: u64,
    pub new_shadow_exposure_allowed: bool,
    pub authority_granted: bool,
    pub nested_runtime_digest: [u8; 32],
    pub nested_cash_reserved_micros: i128,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("shadow gateway configuration is invalid")]
    Config,
    #[error("shadow gateway timestamp is invalid or regressed")]
    Timestamp,
    #[error("shadow gateway command exceeds its canonical bound")]
    CommandBound,
    #[error("shadow gateway command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported shadow gateway command version: {0}")]
    Version(u16),
    #[error("shadow gateway command id was reused for different content")]
    IdempotencyConflict,
    #[error("shadow gateway evidence identity was reused or substituted")]
    Identity,
    #[error("shadow gateway evidence history regressed or equivocated")]
    History,
    #[error("shadow gateway certification evidence is invalid")]
    Certification,
    #[error("shadow gateway fixture evidence is invalid")]
    Fixture,
    #[error("shadow gateway heartbeat evidence is invalid")]
    Heartbeat,
    #[error("nested Phase 2.14 runtime failed: {0}")]
    Runtime(String),
    #[error("shadow gateway arithmetic or counter overflow")]
    Overflow,
    #[error("shadow gateway is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct ShadowGatewayHarness {
    config: GatewayConfig,
    runtime: UnifiedPairedTradingRuntime,
    report: Option<CertificationReport>,
    reports: BTreeMap<[u8; 32], [u8; 32]>,
    heartbeat: Option<StackHeartbeat>,
    fixtures: BTreeMap<FixtureId, RecordedFixture>,
    fixture_sequences: BTreeMap<u64, FixtureId>,
    mode: GatewayMode,
    mode_sequence: u64,
    recovery_epoch: u64,
    processed: BTreeMap<GatewayCommandId, ([u8; 32], GatewayOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl ShadowGatewayHarness {
    /// Creates an empty credentialless harness.
    ///
    /// # Errors
    ///
    /// Rejects invalid gateway or Phase 2.14 reconciliation configuration.
    pub fn new(
        config: GatewayConfig,
        reconciliation: settlement_reconciliation::ReconcilerConfig,
    ) -> Result<Self, Error> {
        validate_config(&config)?;
        let runtime = UnifiedPairedTradingRuntime::new(reconciliation)
            .map_err(|error| Error::Runtime(error.to_string()))?;
        Ok(Self {
            config,
            runtime,
            report: None,
            reports: BTreeMap::new(),
            heartbeat: None,
            fixtures: BTreeMap::new(),
            fixture_sequences: BTreeMap::new(),
            mode: GatewayMode::Starting,
            mode_sequence: 0,
            recovery_epoch: 0,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic top-level shadow command.
    ///
    /// # Errors
    ///
    /// Identity, history, nested-integrity, arithmetic, and durable failures halt.
    pub fn apply(&mut self, command: &GatewayCommand) -> Result<GatewayOutcome, Error> {
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
        let mut outcome = GatewayOutcome {
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

    fn apply_fresh(&mut self, command: &GatewayCommand) -> Result<GatewayDetail, Error> {
        let at = command.recorded_at_ns();
        match command {
            GatewayCommand::InstallCertification {
                command_id, report, ..
            } => self.install_report(*command_id, report, at),
            GatewayCommand::ObserveHeartbeat {
                command_id,
                heartbeat,
                ..
            } => self.observe_heartbeat(*command_id, heartbeat, at),
            GatewayCommand::ApplyFixture {
                command_id,
                fixture,
                ..
            } => self.apply_fixture(*command_id, fixture, at),
            GatewayCommand::ApplyRuntime {
                command_id,
                command: nested,
                ..
            } => self.apply_runtime(*command_id, nested, at),
            GatewayCommand::Recover {
                command_id,
                recovery_epoch,
                reconciliation_current,
                unknown_orders_cleared,
                recovery_evidence_digest,
                ..
            } => self.recover(
                *command_id,
                *recovery_epoch,
                *reconciliation_current,
                *unknown_orders_cleared,
                *recovery_evidence_digest,
                at,
            ),
            GatewayCommand::Tick {
                command_id, now_ns, ..
            } => self.tick(*command_id, *now_ns, at),
        }
    }

    fn install_report(
        &mut self,
        id: GatewayCommandId,
        report: &CertificationReport,
        at: i64,
    ) -> Result<GatewayDetail, Error> {
        if !report.verify_digest()
            || report.authority_granted
            || report.evaluated_at_ns < 0
            || report.evaluated_at_ns > at
        {
            return Err(Error::Certification);
        }
        if let Some(existing) = self.reports.get(&report.profile_id) {
            if existing != &report.report_digest {
                return Err(Error::Identity);
            }
        }
        if self
            .report
            .as_ref()
            .is_some_and(|old| report.evaluated_at_ns < old.evaluated_at_ns)
        {
            return Err(Error::History);
        }
        self.reports.insert(report.profile_id, report.report_digest);
        self.report = Some(report.clone());
        let mut derived = Vec::new();
        self.refresh_mode(id, at, &mut derived)?;
        Ok(GatewayDetail::CertificationInstalled {
            placement_eligible: self.new_exposure_denial(at).is_none(),
            derived,
        })
    }

    fn observe_heartbeat(
        &mut self,
        id: GatewayCommandId,
        heartbeat: &StackHeartbeat,
        at: i64,
    ) -> Result<GatewayDetail, Error> {
        if !heartbeat.verify_digest()
            || heartbeat.sequence == 0
            || heartbeat.observed_at_ns < 0
            || heartbeat.observed_at_ns > at
            || heartbeat.valid_until_ns <= heartbeat.observed_at_ns
        {
            return Err(Error::Heartbeat);
        }
        if self.heartbeat.as_ref().is_some_and(|old| {
            heartbeat.sequence <= old.sequence || heartbeat.observed_at_ns < old.observed_at_ns
        }) {
            return Err(Error::History);
        }
        self.heartbeat = Some(heartbeat.clone());
        let mut derived = Vec::new();
        let dead_man = !heartbeat.completely_healthy();
        if dead_man {
            self.mode = GatewayMode::DeadManTriggered;
            derived.push(self.observe_derived_mode(id, 0, ExchangeMode::TradingDisabled, at)?);
        } else {
            self.refresh_mode(id, at, &mut derived)?;
        }
        Ok(GatewayDetail::HeartbeatObserved {
            dead_man_triggered: dead_man,
            derived,
        })
    }

    fn apply_fixture(
        &mut self,
        id: GatewayCommandId,
        fixture: &RecordedFixture,
        at: i64,
    ) -> Result<GatewayDetail, Error> {
        if fixture.contract_digest != self.config.expected_contract_digest
            || fixture.sequence == 0
            || fixture.payload_digest == [0; 32]
            || fixture.captured_at_ns < 0
            || fixture.received_at_ns < fixture.captured_at_ns
            || fixture.received_at_ns > at
        {
            return Err(Error::Fixture);
        }
        if self.fixtures.contains_key(&fixture.fixture_id)
            || self.fixture_sequences.contains_key(&fixture.sequence)
            || self
                .fixture_sequences
                .last_key_value()
                .is_some_and(|(sequence, _)| fixture.sequence <= *sequence)
        {
            return Err(Error::History);
        }
        self.fixtures.insert(fixture.fixture_id, fixture.clone());
        self.fixture_sequences
            .insert(fixture.sequence, fixture.fixture_id);
        let (translation, mode, exchange_mode) = fixture_translation(fixture.kind);
        self.mode = mode;
        if fixture.kind == FixtureKind::HeartbeatLost {
            self.heartbeat = None;
        }
        let mut derived = Vec::new();
        if let Some(exchange_mode) = exchange_mode {
            derived.push(self.observe_derived_mode(id, 0, exchange_mode, at)?);
        }
        Ok(GatewayDetail::FixtureApplied {
            translation,
            automatic_retry: false,
            backing_released: false,
            derived,
        })
    }

    fn apply_runtime(
        &mut self,
        id: GatewayCommandId,
        command: &UnifiedCommand,
        at: i64,
    ) -> Result<GatewayDetail, Error> {
        if command.recorded_at_ns() != at {
            return Err(Error::Timestamp);
        }
        if matches!(command, UnifiedCommand::ObserveMode { .. }) {
            return Ok(GatewayDetail::RuntimeDenied {
                reason: GatewayDenial::CallerModeObservationForbidden,
                derived: Vec::new(),
            });
        }
        if increases_shadow_exposure(command) {
            if let Some(reason) = self.new_exposure_denial(at) {
                let mut derived = Vec::new();
                match reason {
                    GatewayDenial::CertificationExpired => {
                        self.mode = GatewayMode::CertificationExpired;
                        derived.push(self.observe_derived_mode(
                            id,
                            0,
                            ExchangeMode::TradingDisabled,
                            at,
                        )?);
                    }
                    GatewayDenial::HeartbeatExpired | GatewayDenial::HeartbeatUnhealthy => {
                        self.mode = GatewayMode::DeadManTriggered;
                        derived.push(self.observe_derived_mode(
                            id,
                            0,
                            ExchangeMode::TradingDisabled,
                            at,
                        )?);
                    }
                    _ => {}
                }
                return Ok(GatewayDetail::RuntimeDenied { reason, derived });
            }
        }
        let outcome = self
            .runtime
            .apply(command)
            .map_err(|error| Error::Runtime(error.to_string()))?;
        Ok(GatewayDetail::RuntimeApplied(outcome))
    }

    fn recover(
        &mut self,
        id: GatewayCommandId,
        epoch: u64,
        reconciliation_current: bool,
        unknown_orders_cleared: bool,
        recovery_evidence_digest: [u8; 32],
        at: i64,
    ) -> Result<GatewayDetail, Error> {
        if epoch <= self.recovery_epoch {
            return Err(Error::History);
        }
        if !matches!(
            self.mode,
            GatewayMode::Restarting
                | GatewayMode::Recovering
                | GatewayMode::Backoff
                | GatewayMode::BackingRetained
                | GatewayMode::DeadManTriggered
                | GatewayMode::DelayActive
        ) {
            return Ok(GatewayDetail::RecoveryDenied(GatewayDenial::GatewayMode));
        }
        if !reconciliation_current
            || !self.runtime.ctf().parent().reconciliation_is_current()
            || !unknown_orders_cleared
            || recovery_evidence_digest == [0; 32]
        {
            return Ok(GatewayDetail::RecoveryDenied(
                GatewayDenial::RecoveryEvidenceMissing,
            ));
        }
        if let Some(reason) = certification_denial(&self.config, self.report.as_ref(), at) {
            return Ok(GatewayDetail::RecoveryDenied(reason));
        }
        if let Some(reason) = heartbeat_denial(&self.config, self.heartbeat.as_ref(), at) {
            return Ok(GatewayDetail::RecoveryDenied(reason));
        }
        let mut derived = vec![self.observe_derived_mode(id, 0, ExchangeMode::Recovering, at)?];
        derived.push(self.observe_derived_mode(id, 1, ExchangeMode::Normal, at)?);
        self.recovery_epoch = epoch;
        self.mode = GatewayMode::Ready;
        Ok(GatewayDetail::Recovered {
            recovery_epoch: epoch,
            derived,
        })
    }

    fn tick(&mut self, id: GatewayCommandId, now: i64, at: i64) -> Result<GatewayDetail, Error> {
        if now < 0 || now != at {
            return Err(Error::Timestamp);
        }
        let heartbeat_failed = heartbeat_denial(&self.config, self.heartbeat.as_ref(), now)
            .is_some_and(|reason| {
                matches!(
                    reason,
                    GatewayDenial::HeartbeatUnhealthy | GatewayDenial::HeartbeatExpired
                )
            });
        let certification_expired = matches!(
            certification_denial(&self.config, self.report.as_ref(), now),
            Some(GatewayDenial::CertificationExpired)
        );
        let mut derived = Vec::new();
        if heartbeat_failed {
            self.mode = GatewayMode::DeadManTriggered;
            derived.push(self.observe_derived_mode(id, 0, ExchangeMode::TradingDisabled, at)?);
        } else if certification_expired {
            self.mode = GatewayMode::CertificationExpired;
            derived.push(self.observe_derived_mode(id, 0, ExchangeMode::TradingDisabled, at)?);
        }
        Ok(GatewayDetail::TickApplied {
            dead_man_triggered: heartbeat_failed,
            certification_expired,
            derived,
        })
    }

    fn refresh_mode(
        &mut self,
        id: GatewayCommandId,
        at: i64,
        derived: &mut Vec<UnifiedOutcome>,
    ) -> Result<(), Error> {
        let denial = certification_denial(&self.config, self.report.as_ref(), at)
            .or_else(|| heartbeat_denial(&self.config, self.heartbeat.as_ref(), at));
        match denial {
            None => {
                self.mode = GatewayMode::Ready;
                derived.push(self.observe_derived_mode(id, 0, ExchangeMode::Normal, at)?);
            }
            Some(GatewayDenial::CertificationExpired) => {
                self.mode = GatewayMode::CertificationExpired;
                derived.push(self.observe_derived_mode(
                    id,
                    0,
                    ExchangeMode::TradingDisabled,
                    at,
                )?);
            }
            Some(GatewayDenial::HeartbeatUnhealthy | GatewayDenial::HeartbeatExpired) => {
                self.mode = GatewayMode::DeadManTriggered;
                derived.push(self.observe_derived_mode(
                    id,
                    0,
                    ExchangeMode::TradingDisabled,
                    at,
                )?);
            }
            Some(_) => {
                self.mode = GatewayMode::CertificationRequired;
                derived.push(self.observe_derived_mode(
                    id,
                    0,
                    ExchangeMode::TradingDisabled,
                    at,
                )?);
            }
        }
        Ok(())
    }

    fn observe_derived_mode(
        &mut self,
        parent: GatewayCommandId,
        substep: u8,
        mode: ExchangeMode,
        at: i64,
    ) -> Result<UnifiedOutcome, Error> {
        self.mode_sequence = self.mode_sequence.checked_add(1).ok_or(Error::Overflow)?;
        let valid_until_ns = at
            .checked_add(self.config.mode_validity_ns)
            .ok_or(Error::Overflow)?;
        let command = UnifiedCommand::ObserveMode {
            command_id: derived_unified_id(parent, substep, self.mode_sequence),
            observation: ExchangeModeObservation {
                sequence: self.mode_sequence,
                mode,
                observed_at_ns: at,
                valid_until_ns,
            },
            recorded_at_ns: at,
        };
        self.runtime
            .apply(&command)
            .map_err(|error| Error::Runtime(error.to_string()))
    }

    fn new_exposure_denial(&self, at: i64) -> Option<GatewayDenial> {
        certification_denial(&self.config, self.report.as_ref(), at)
            .or_else(|| heartbeat_denial(&self.config, self.heartbeat.as_ref(), at))
            .or_else(|| {
                (!matches!(self.mode, GatewayMode::Ready | GatewayMode::PostOnly))
                    .then_some(GatewayDenial::GatewayMode)
            })
    }

    #[must_use]
    pub const fn runtime(&self) -> &UnifiedPairedTradingRuntime {
        &self.runtime
    }

    #[must_use]
    pub fn snapshot(&self) -> GatewaySnapshot {
        let at = self.last_recorded_at_ns.unwrap_or(0);
        let nested = self.runtime.snapshot();
        let certification_fresh =
            certification_denial(&self.config, self.report.as_ref(), at).is_none();
        let heartbeat_healthy =
            heartbeat_denial(&self.config, self.heartbeat.as_ref(), at).is_none();
        GatewaySnapshot {
            accepted_commands: self.accepted_commands,
            mode: if self.halted.is_some() {
                GatewayMode::Halted
            } else {
                self.mode
            },
            certification_report_digest: self.report.as_ref().map(|value| value.report_digest),
            certification_fresh,
            heartbeat_healthy,
            fixture_count: self.fixtures.len(),
            recovery_epoch: self.recovery_epoch,
            mode_sequence: self.mode_sequence,
            new_shadow_exposure_allowed: self.new_exposure_denial(at).is_none(),
            authority_granted: false,
            nested_runtime_digest: nested.digest,
            nested_cash_reserved_micros: nested.cash_reserved_micros,
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
        hasher.update(b"shadow-gateway-harness-state-v1");
        hash_json(&mut hasher, &self.config);
        hasher.update(&self.runtime.snapshot().digest);
        hash_json(&mut hasher, &self.report);
        for (profile_id, report_digest) in &self.reports {
            hasher.update(profile_id);
            hasher.update(report_digest);
        }
        hash_json(&mut hasher, &self.heartbeat);
        for (id, fixture) in &self.fixtures {
            hasher.update(&id.0);
            hash_json(&mut hasher, fixture);
        }
        hash_json(&mut hasher, &self.fixture_sequences);
        hash_json(&mut hasher, &self.mode);
        hash_json(&mut hasher, &self.mode_sequence);
        hash_json(&mut hasher, &self.recovery_epoch);
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

fn validate_config(config: &GatewayConfig) -> Result<(), Error> {
    if config.expected_contract_digest == [0; 32]
        || config.certification_max_age_ns <= 0
        || config.heartbeat_max_age_ns <= 0
        || config.mode_validity_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn certification_denial(
    config: &GatewayConfig,
    report: Option<&CertificationReport>,
    at: i64,
) -> Option<GatewayDenial> {
    let Some(report) = report else {
        return Some(GatewayDenial::CertificationMissing);
    };
    if report.status != CertificationStatus::Certified || !report.reasons.is_empty() {
        return Some(GatewayDenial::CertificationNotCertified);
    }
    if report.authority_granted
        || report.contract_digest != Some(config.expected_contract_digest)
        || !report.verify_digest()
    {
        return Some(GatewayDenial::CertificationMismatch);
    }
    if at < report.evaluated_at_ns || at - report.evaluated_at_ns > config.certification_max_age_ns
    {
        return Some(GatewayDenial::CertificationExpired);
    }
    None
}

fn heartbeat_denial(
    config: &GatewayConfig,
    heartbeat: Option<&StackHeartbeat>,
    at: i64,
) -> Option<GatewayDenial> {
    let Some(heartbeat) = heartbeat else {
        return Some(GatewayDenial::HeartbeatMissing);
    };
    if !heartbeat.completely_healthy() {
        return Some(GatewayDenial::HeartbeatUnhealthy);
    }
    if at < heartbeat.observed_at_ns
        || at > heartbeat.valid_until_ns
        || at - heartbeat.observed_at_ns > config.heartbeat_max_age_ns
    {
        return Some(GatewayDenial::HeartbeatExpired);
    }
    None
}

const fn fixture_translation(
    kind: FixtureKind,
) -> (FixtureTranslation, GatewayMode, Option<ExchangeMode>) {
    match kind {
        FixtureKind::Restart425 => (
            FixtureTranslation::EngineRestarting,
            GatewayMode::Restarting,
            Some(ExchangeMode::Restarting),
        ),
        FixtureKind::PostOnlyWindow => (
            FixtureTranslation::PostOnlyWindow,
            GatewayMode::PostOnly,
            Some(ExchangeMode::PostOnly),
        ),
        FixtureKind::CancelOnlyMode => (
            FixtureTranslation::CancelOnlyWindow,
            GatewayMode::CancelOnly,
            Some(ExchangeMode::CancelOnly),
        ),
        FixtureKind::TakerDelay => (
            FixtureTranslation::TakerDelayActive,
            GatewayMode::DelayActive,
            Some(ExchangeMode::Recovering),
        ),
        FixtureKind::TickSizeChange => (
            FixtureTranslation::RevalidatePricesAndOrders,
            GatewayMode::Recovering,
            Some(ExchangeMode::Recovering),
        ),
        FixtureKind::RateLimit429 => (
            FixtureTranslation::RateLimitedBackoff,
            GatewayMode::Backoff,
            Some(ExchangeMode::Recovering),
        ),
        FixtureKind::UnknownOrder => (
            FixtureTranslation::UnknownOrderReconciliation,
            GatewayMode::BackingRetained,
            Some(ExchangeMode::Recovering),
        ),
        FixtureKind::SettlementRetrying => (
            FixtureTranslation::SettlementValueRetained,
            GatewayMode::BackingRetained,
            Some(ExchangeMode::Recovering),
        ),
        FixtureKind::HeartbeatLost => (
            FixtureTranslation::DeadManCancelAndDisable,
            GatewayMode::DeadManTriggered,
            Some(ExchangeMode::TradingDisabled),
        ),
    }
}

const fn increases_shadow_exposure(command: &UnifiedCommand) -> bool {
    matches!(
        command,
        UnifiedCommand::EvaluateAndStage { .. }
            | UnifiedCommand::AuthorizeAndSubmitFirst { .. }
            | UnifiedCommand::AuthorizeAndSubmitHedge { .. }
            | UnifiedCommand::RequestConversion { .. }
    )
}

fn derived_unified_id(parent: GatewayCommandId, substep: u8, sequence: u64) -> UnifiedCommandId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"shadow-gateway-derived-mode-v1");
    hasher.update(&parent.0);
    hasher.update(&[substep]);
    hasher.update(&sequence.to_le_bytes());
    UnifiedCommandId(*hasher.finalize().as_bytes())
}

fn heartbeat_digest(heartbeat: &StackHeartbeat) -> [u8; 32] {
    let mut clone = heartbeat.clone();
    clone.observation_digest = [0; 32];
    digest_json(b"shadow-gateway-heartbeat-v1", &clone)
}

fn outcome_digest(outcome: &GatewayOutcome) -> [u8; 32] {
    let mut clone = outcome.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"shadow-gateway-outcome-v1", &clone)
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
    command: GatewayCommand,
}

/// Encodes one bounded versioned gateway command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &GatewayCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one bounded versioned gateway command.
///
/// # Errors
///
/// Rejects malformed, oversized, trailing, or unsupported input.
pub fn decode_command(bytes: &[u8]) -> Result<GatewayCommand, Error> {
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
