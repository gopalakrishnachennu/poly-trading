#![forbid(unsafe_code)]

//! Deterministic operational safety and stress gates for read-only shadow mode.

use event_schema::EventEnvelope;
use integration_daemon::{run_soak, FaultScript, IntegrationError, SoakPlan};
use market_recorder::{EventJournal, JournalBackendError, JournalError, SegmentError};
use market_session::MarketSessionCoordinator;
use session_runtime::{
    DurableCoordinator, RecoveryState, RuntimeError, RuntimeMode, RuntimeSnapshot,
};
use std::cell::Cell;
use std::time::Duration;
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct OperationalConfig {
    pub watchdog_timeout: Duration,
    pub max_rss_bytes: u64,
    pub max_open_files: u64,
    pub max_journal_bytes: u64,
    pub max_ingress_depth: u64,
    pub max_tick_duration: Duration,
}

impl Default for OperationalConfig {
    fn default() -> Self {
        Self {
            watchdog_timeout: Duration::from_secs(10),
            max_rss_bytes: 2 * 1024 * 1024 * 1024,
            max_open_files: 4_096,
            max_journal_bytes: 100 * 1024 * 1024 * 1024,
            max_ingress_depth: 768,
            max_tick_duration: Duration::from_secs(2),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationalSample {
    pub now_ns: i64,
    pub last_progress_ns: Option<i64>,
    pub runtime_mode: RuntimeMode,
    pub coordinator_halted: bool,
    pub last_sequence: Option<u64>,
    pub ingress_depth: u64,
    pub ingress_capacity: u64,
    pub rss_bytes: u64,
    pub open_files: u64,
    pub journal_bytes: u64,
    pub tick_duration_ns: u64,
}

impl OperationalSample {
    #[must_use]
    pub fn from_runtime(
        now_ns: i64,
        last_progress_ns: Option<i64>,
        runtime: &RuntimeSnapshot,
        resources: ResourceSample,
    ) -> Self {
        Self {
            now_ns,
            last_progress_ns,
            runtime_mode: runtime.mode,
            coordinator_halted: runtime.coordinator.halted,
            last_sequence: runtime.last_sequence,
            ingress_depth: resources.ingress_depth,
            ingress_capacity: resources.ingress_capacity,
            rss_bytes: resources.rss_bytes,
            open_files: resources.open_files,
            journal_bytes: resources.journal_bytes,
            tick_duration_ns: resources.tick_duration_ns,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceSample {
    pub ingress_depth: u64,
    pub ingress_capacity: u64,
    pub rss_bytes: u64,
    pub open_files: u64,
    pub journal_bytes: u64,
    pub tick_duration_ns: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum OperationalMode {
    Starting = 1,
    Ready = 2,
    Degraded = 3,
    Draining = 4,
    Stopped = 5,
    Halted = 6,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum OperationalReason {
    Healthy = 1,
    RssBudget = 2,
    OpenFileBudget = 3,
    JournalBudget = 4,
    IngressWatermark = 5,
    TickLatency = 6,
    RuntimeHalted = 7,
    CoordinatorHalted = 8,
    ClockRegression = 9,
    SequenceRegression = 10,
    ImpossibleIngressDepth = 11,
    FutureProgress = 12,
    MissingProgress = 13,
    WatchdogExpired = 14,
    DrainRequested = 15,
    ProcessStopped = 16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OperationalCounters {
    pub evaluations: u64,
    pub ready_evaluations: u64,
    pub degraded_evaluations: u64,
    pub integrity_halts: u64,
    pub drain_requests: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationalSnapshot {
    pub mode: OperationalMode,
    pub reason: OperationalReason,
    pub evaluated_at_ns: Option<i64>,
    pub last_progress_ns: Option<i64>,
    pub last_sequence: Option<u64>,
    pub resources: Option<ResourceSample>,
    pub counters: OperationalCounters,
    pub halt_detail: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct OperationalSupervisor {
    watchdog_ns: i64,
    max_rss_bytes: u64,
    max_open_files: u64,
    max_journal_bytes: u64,
    max_ingress_depth: u64,
    max_tick_ns: u64,
    mode: OperationalMode,
    reason: OperationalReason,
    last_now_ns: Option<i64>,
    last_progress_ns: Option<i64>,
    last_sequence: Option<u64>,
    resources: Option<ResourceSample>,
    counters: OperationalCounters,
    halt_detail: Option<String>,
}

impl OperationalSupervisor {
    /// Creates a deterministic operational supervisor.
    ///
    /// # Errors
    ///
    /// Rejects zero or unrepresentable watchdog, resource, and latency limits.
    pub fn new(config: OperationalConfig) -> Result<Self, OperationalError> {
        let watchdog_ns = i64::try_from(config.watchdog_timeout.as_nanos())
            .map_err(|_| OperationalError::InvalidConfig)?;
        let max_tick_ns = u64::try_from(config.max_tick_duration.as_nanos())
            .map_err(|_| OperationalError::InvalidConfig)?;
        if watchdog_ns == 0
            || max_tick_ns == 0
            || config.max_rss_bytes == 0
            || config.max_open_files == 0
            || config.max_journal_bytes == 0
            || config.max_ingress_depth == 0
        {
            return Err(OperationalError::InvalidConfig);
        }
        Ok(Self {
            watchdog_ns,
            max_rss_bytes: config.max_rss_bytes,
            max_open_files: config.max_open_files,
            max_journal_bytes: config.max_journal_bytes,
            max_ingress_depth: config.max_ingress_depth,
            max_tick_ns,
            mode: OperationalMode::Starting,
            reason: OperationalReason::Healthy,
            last_now_ns: None,
            last_progress_ns: None,
            last_sequence: None,
            resources: None,
            counters: OperationalCounters::default(),
            halt_detail: None,
        })
    }

    /// Evaluates one explicit runtime and resource sample.
    ///
    /// # Errors
    ///
    /// Integrity failures permanently halt. Calls after halt return `Halted`.
    pub fn evaluate(
        &mut self,
        sample: OperationalSample,
    ) -> Result<OperationalSnapshot, OperationalError> {
        if let Some(detail) = &self.halt_detail {
            return Err(OperationalError::Halted(detail.clone()));
        }
        if sample.now_ns < 0 {
            return self.halt(
                OperationalReason::ClockRegression,
                "negative evaluation time",
            );
        }
        if self.last_now_ns.is_some_and(|value| sample.now_ns < value) {
            return self.halt(
                OperationalReason::ClockRegression,
                "evaluation clock regressed",
            );
        }
        if sequence_regressed(self.last_sequence, sample.last_sequence) {
            return self.halt(
                OperationalReason::SequenceRegression,
                "runtime sequence regressed",
            );
        }
        if sample.ingress_capacity == 0 || sample.ingress_depth > sample.ingress_capacity {
            return self.halt(
                OperationalReason::ImpossibleIngressDepth,
                "ingress depth exceeds capacity",
            );
        }
        if sample
            .last_progress_ns
            .is_some_and(|progress| progress < 0 || progress > sample.now_ns)
        {
            return self.halt(
                OperationalReason::FutureProgress,
                "progress time is invalid",
            );
        }
        if sample.runtime_mode == RuntimeMode::Halted {
            return self.halt(OperationalReason::RuntimeHalted, "session runtime halted");
        }
        if sample.coordinator_halted {
            return self.halt(
                OperationalReason::CoordinatorHalted,
                "session coordinator halted",
            );
        }

        self.counters.evaluations = checked_counter(self.counters.evaluations)?;
        self.last_now_ns = Some(sample.now_ns);
        self.last_progress_ns = sample.last_progress_ns;
        self.last_sequence = sample.last_sequence;
        self.resources = Some(ResourceSample {
            ingress_depth: sample.ingress_depth,
            ingress_capacity: sample.ingress_capacity,
            rss_bytes: sample.rss_bytes,
            open_files: sample.open_files,
            journal_bytes: sample.journal_bytes,
            tick_duration_ns: sample.tick_duration_ns,
        });

        if matches!(
            sample.runtime_mode,
            RuntimeMode::Closed | RuntimeMode::Shutdown
        ) {
            self.mode = OperationalMode::Stopped;
            self.reason = OperationalReason::ProcessStopped;
            return Ok(self.snapshot());
        }
        if self.mode == OperationalMode::Stopped {
            return Err(OperationalError::Stopped);
        }
        if self.mode == OperationalMode::Draining {
            self.reason = OperationalReason::DrainRequested;
            return Ok(self.snapshot());
        }
        let Some(progress) = sample.last_progress_ns else {
            return self.halt(OperationalReason::MissingProgress, "progress is missing");
        };
        let age = sample
            .now_ns
            .checked_sub(progress)
            .ok_or(OperationalError::TimestampOverflow)?;
        if age > self.watchdog_ns {
            return self.halt(OperationalReason::WatchdogExpired, "watchdog expired");
        }
        if let Some(reason) = self.budget_reason(&sample) {
            self.mode = OperationalMode::Degraded;
            self.reason = reason;
            self.counters.degraded_evaluations =
                checked_counter(self.counters.degraded_evaluations)?;
        } else {
            self.mode = OperationalMode::Ready;
            self.reason = OperationalReason::Healthy;
            self.counters.ready_evaluations = checked_counter(self.counters.ready_evaluations)?;
        }
        Ok(self.snapshot())
    }

    /// Begins graceful drain and prevents future ready transitions.
    ///
    /// # Errors
    ///
    /// Rejects drain after halt or stop.
    pub fn begin_drain(&mut self) -> Result<OperationalSnapshot, OperationalError> {
        if let Some(detail) = &self.halt_detail {
            return Err(OperationalError::Halted(detail.clone()));
        }
        if self.mode == OperationalMode::Stopped {
            return Err(OperationalError::Stopped);
        }
        self.mode = OperationalMode::Draining;
        self.reason = OperationalReason::DrainRequested;
        self.counters.drain_requests = checked_counter(self.counters.drain_requests)?;
        Ok(self.snapshot())
    }

    /// Marks a drained process stopped.
    ///
    /// # Errors
    ///
    /// Requires the draining state and rejects halt.
    pub fn mark_stopped(&mut self) -> Result<OperationalSnapshot, OperationalError> {
        if let Some(detail) = &self.halt_detail {
            return Err(OperationalError::Halted(detail.clone()));
        }
        if self.mode != OperationalMode::Draining {
            return Err(OperationalError::NotDraining);
        }
        self.mode = OperationalMode::Stopped;
        self.reason = OperationalReason::ProcessStopped;
        Ok(self.snapshot())
    }

    #[must_use]
    pub fn snapshot(&self) -> OperationalSnapshot {
        let digest = operational_digest(self);
        OperationalSnapshot {
            mode: self.mode,
            reason: self.reason,
            evaluated_at_ns: self.last_now_ns,
            last_progress_ns: self.last_progress_ns,
            last_sequence: self.last_sequence,
            resources: self.resources,
            counters: self.counters,
            halt_detail: self.halt_detail.clone(),
            digest,
        }
    }

    fn budget_reason(&self, sample: &OperationalSample) -> Option<OperationalReason> {
        if sample.rss_bytes > self.max_rss_bytes {
            Some(OperationalReason::RssBudget)
        } else if sample.open_files > self.max_open_files {
            Some(OperationalReason::OpenFileBudget)
        } else if sample.journal_bytes > self.max_journal_bytes {
            Some(OperationalReason::JournalBudget)
        } else if sample.ingress_depth > self.max_ingress_depth {
            Some(OperationalReason::IngressWatermark)
        } else if sample.tick_duration_ns > self.max_tick_ns {
            Some(OperationalReason::TickLatency)
        } else {
            None
        }
    }

    fn halt<T>(
        &mut self,
        reason: OperationalReason,
        detail: &'static str,
    ) -> Result<T, OperationalError> {
        self.mode = OperationalMode::Halted;
        self.reason = reason;
        self.halt_detail = Some(detail.to_owned());
        self.counters.integrity_halts = self.counters.integrity_halts.saturating_add(1);
        Err(OperationalError::Integrity(reason))
    }
}

fn sequence_regressed(previous: Option<u64>, next: Option<u64>) -> bool {
    matches!((previous, next), (Some(_), None))
        || matches!((previous, next), (Some(left), Some(right)) if right < left)
}

fn checked_counter(value: u64) -> Result<u64, OperationalError> {
    value
        .checked_add(1)
        .ok_or(OperationalError::CounterOverflow)
}

fn operational_digest(state: &OperationalSupervisor) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"POLY_SHADOW_OPS_V1");
    hasher.update(&[state.mode as u8, state.reason as u8]);
    hasher.update(&state.last_now_ns.unwrap_or(i64::MIN).to_le_bytes());
    hasher.update(&state.last_progress_ns.unwrap_or(i64::MIN).to_le_bytes());
    hasher.update(&state.last_sequence.unwrap_or(u64::MAX).to_le_bytes());
    if let Some(resources) = state.resources {
        hasher.update(&[1]);
        for value in [
            resources.ingress_depth,
            resources.ingress_capacity,
            resources.rss_bytes,
            resources.open_files,
            resources.journal_bytes,
            resources.tick_duration_ns,
        ] {
            hasher.update(&value.to_le_bytes());
        }
    } else {
        hasher.update(&[0]);
    }
    for value in [
        state.counters.evaluations,
        state.counters.ready_evaluations,
        state.counters.degraded_evaluations,
        state.counters.integrity_halts,
        state.counters.drain_requests,
    ] {
        hasher.update(&value.to_le_bytes());
    }
    if let Some(detail) = &state.halt_detail {
        hasher.update(&[1]);
        hasher.update(&(detail.len() as u64).to_le_bytes());
        hasher.update(detail.as_bytes());
    } else {
        hasher.update(&[0]);
    }
    *hasher.finalize().as_bytes()
}

/// Renders deterministic OpenMetrics-compatible health without identifiers.
#[must_use]
pub fn render_openmetrics(snapshot: &OperationalSnapshot) -> String {
    let resources = snapshot.resources.unwrap_or(ResourceSample {
        ingress_depth: 0,
        ingress_capacity: 0,
        rss_bytes: 0,
        open_files: 0,
        journal_bytes: 0,
        tick_duration_ns: 0,
    });
    format!(
        "# TYPE poly_shadow_ready gauge\npoly_shadow_ready {}\n# TYPE poly_shadow_degraded gauge\npoly_shadow_degraded {}\n# TYPE poly_shadow_halted gauge\npoly_shadow_halted {}\npoly_shadow_reason_code {}\npoly_shadow_ingress_depth {}\npoly_shadow_ingress_capacity {}\npoly_shadow_rss_bytes {}\npoly_shadow_open_files {}\npoly_shadow_journal_bytes {}\npoly_shadow_tick_duration_ns {}\npoly_shadow_evaluations_total {}\npoly_shadow_ready_evaluations_total {}\npoly_shadow_degraded_evaluations_total {}\npoly_shadow_integrity_halts_total {}\npoly_shadow_drain_requests_total {}\n# EOF\n",
        u8::from(snapshot.mode == OperationalMode::Ready),
        u8::from(snapshot.mode == OperationalMode::Degraded),
        u8::from(snapshot.mode == OperationalMode::Halted),
        snapshot.reason as u8,
        resources.ingress_depth,
        resources.ingress_capacity,
        resources.rss_bytes,
        resources.open_files,
        resources.journal_bytes,
        resources.tick_duration_ns,
        snapshot.counters.evaluations,
        snapshot.counters.ready_evaluations,
        snapshot.counters.degraded_evaluations,
        snapshot.counters.integrity_halts,
        snapshot.counters.drain_requests,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StressProfile {
    Smoke,
    OneDay,
    SevenDay,
}

impl StressProfile {
    #[must_use]
    pub const fn plan(self) -> SoakPlan {
        match self {
            Self::Smoke => SoakPlan {
                start_time_ms: 3_600_000,
                hours: 3,
                ticks_per_hour: 4,
            },
            Self::OneDay => SoakPlan {
                start_time_ms: 3_600_000,
                hours: 24,
                ticks_per_hour: 4,
            },
            Self::SevenDay => SoakPlan {
                start_time_ms: 3_600_000,
                hours: 168,
                ticks_per_hour: 1,
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct StressCountingJournal {
    last_sequence: Option<u64>,
    records: u64,
    encoded_bytes: u64,
    syncs: Cell<u64>,
}

impl StressCountingJournal {
    #[must_use]
    pub const fn records(&self) -> u64 {
        self.records
    }

    #[must_use]
    pub const fn encoded_bytes(&self) -> u64 {
        self.encoded_bytes
    }

    #[must_use]
    pub fn syncs(&self) -> u64 {
        self.syncs.get()
    }
}

impl EventJournal for StressCountingJournal {
    fn append_event(&mut self, event: &EventEnvelope) -> Result<u64, JournalBackendError> {
        let expected = match self.last_sequence {
            Some(value) => value.checked_add(1).ok_or(JournalBackendError::Segmented(
                SegmentError::SequenceExhausted,
            ))?,
            None => 0,
        };
        if event.sequence != expected {
            return Err(JournalBackendError::Segmented(
                if event.sequence < expected {
                    SegmentError::SequenceRegression {
                        expected,
                        actual: event.sequence,
                    }
                } else {
                    SegmentError::SequenceGap {
                        expected,
                        actual: event.sequence,
                    }
                },
            ));
        }
        let encoded = event.encode().map_err(|source| {
            JournalBackendError::Single(JournalError::InvalidEnvelope {
                offset: self.encoded_bytes,
                source,
            })
        })?;
        let length = u64::try_from(encoded.len())
            .map_err(|_| JournalBackendError::Segmented(SegmentError::RecordLengthOverflow))?;
        let offset = self.encoded_bytes;
        self.encoded_bytes = self
            .encoded_bytes
            .checked_add(length)
            .and_then(|value| value.checked_add(8))
            .ok_or(JournalBackendError::Segmented(
                SegmentError::RecordLengthOverflow,
            ))?;
        self.records = self
            .records
            .checked_add(1)
            .ok_or(JournalBackendError::Segmented(
                SegmentError::RecordLengthOverflow,
            ))?;
        self.last_sequence = Some(event.sequence);
        Ok(offset)
    }

    fn sync_events(&self) -> Result<(), JournalBackendError> {
        self.syncs.set(self.syncs.get().saturating_add(1));
        Ok(())
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last_sequence
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StressReport {
    pub profile: StressProfile,
    pub ticks: u64,
    pub sessions: usize,
    pub finalized_sessions: usize,
    pub ready_observations: u64,
    pub records: u64,
    pub encoded_bytes: u64,
    pub syncs: u64,
    pub coordinator_digest: [u8; 32],
}

/// Runs a named bounded stress profile using a non-durable accounting journal.
///
/// # Errors
///
/// Returns integration or runtime failure. This function is a stress tool and
/// never substitutes for the durable segmented-journal tests.
pub fn run_stress_profile(profile: StressProfile) -> Result<StressReport, StressError> {
    let journal = StressCountingJournal::default();
    let durable = DurableCoordinator::new(
        journal,
        RecoveryState {
            coordinator: MarketSessionCoordinator::default(),
            last_sequence: None,
        },
    )?;
    let (soak, mut durable) = run_soak(durable, profile.plan(), FaultScript::default())?;
    durable.sync()?;
    let journal = durable.journal();
    Ok(StressReport {
        profile,
        ticks: soak.ticks,
        sessions: soak.generated_sessions,
        finalized_sessions: soak.finalized_sessions,
        ready_observations: soak.ready_session_observations,
        records: journal.records(),
        encoded_bytes: journal.encoded_bytes(),
        syncs: journal.syncs(),
        coordinator_digest: soak.coordinator_digest,
    })
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum OperationalError {
    #[error("operational configuration is invalid")]
    InvalidConfig,
    #[error("operational timestamp arithmetic overflow")]
    TimestampOverflow,
    #[error("operational counter overflow")]
    CounterOverflow,
    #[error("operational integrity halt: {0:?}")]
    Integrity(OperationalReason),
    #[error("operational supervisor is halted: {0}")]
    Halted(String),
    #[error("operational process is stopped")]
    Stopped,
    #[error("operational process is not draining")]
    NotDraining,
}

#[derive(Debug, Error)]
pub enum StressError {
    #[error("stress integration failed: {0}")]
    Integration(#[from] IntegrationError),
    #[error("stress runtime failed: {0}")]
    Runtime(#[from] RuntimeError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> OperationalConfig {
        OperationalConfig {
            watchdog_timeout: Duration::from_nanos(100),
            max_rss_bytes: 100,
            max_open_files: 100,
            max_journal_bytes: 100,
            max_ingress_depth: 8,
            max_tick_duration: Duration::from_nanos(100),
        }
    }

    fn sample(now_ns: i64, sequence: u64) -> OperationalSample {
        OperationalSample {
            now_ns,
            last_progress_ns: Some(now_ns),
            runtime_mode: RuntimeMode::Ready,
            coordinator_halted: false,
            last_sequence: Some(sequence),
            ingress_depth: 8,
            ingress_capacity: 10,
            rss_bytes: 100,
            open_files: 100,
            journal_bytes: 100,
            tick_duration_ns: 100,
        }
    }

    #[test]
    fn exact_budgets_are_ready_and_excess_recovers() {
        let mut supervisor = OperationalSupervisor::new(config()).expect("supervisor");
        let ready = supervisor.evaluate(sample(100, 0)).expect("ready");
        assert_eq!(ready.mode, OperationalMode::Ready);
        assert_eq!(ready.reason, OperationalReason::Healthy);

        let mut exceeded = sample(101, 1);
        exceeded.rss_bytes = 101;
        let degraded = supervisor.evaluate(exceeded).expect("degraded");
        assert_eq!(degraded.mode, OperationalMode::Degraded);
        assert_eq!(degraded.reason, OperationalReason::RssBudget);

        let recovered = supervisor.evaluate(sample(102, 2)).expect("recovered");
        assert_eq!(recovered.mode, OperationalMode::Ready);
        assert_eq!(recovered.counters.ready_evaluations, 2);
        assert_eq!(recovered.counters.degraded_evaluations, 1);
    }

    #[test]
    fn every_resource_budget_has_an_explicit_reason() {
        let cases = [
            (
                OperationalReason::OpenFileBudget,
                ResourceSample {
                    ingress_depth: 8,
                    ingress_capacity: 10,
                    rss_bytes: 100,
                    open_files: 101,
                    journal_bytes: 100,
                    tick_duration_ns: 100,
                },
            ),
            (
                OperationalReason::JournalBudget,
                ResourceSample {
                    ingress_depth: 8,
                    ingress_capacity: 10,
                    rss_bytes: 100,
                    open_files: 100,
                    journal_bytes: 101,
                    tick_duration_ns: 100,
                },
            ),
            (
                OperationalReason::IngressWatermark,
                ResourceSample {
                    ingress_depth: 9,
                    ingress_capacity: 10,
                    rss_bytes: 100,
                    open_files: 100,
                    journal_bytes: 100,
                    tick_duration_ns: 100,
                },
            ),
            (
                OperationalReason::TickLatency,
                ResourceSample {
                    ingress_depth: 8,
                    ingress_capacity: 10,
                    rss_bytes: 100,
                    open_files: 100,
                    journal_bytes: 100,
                    tick_duration_ns: 101,
                },
            ),
        ];
        for (reason, resources) in cases {
            let mut supervisor = OperationalSupervisor::new(config()).expect("supervisor");
            let mut value = sample(100, 0);
            value.ingress_depth = resources.ingress_depth;
            value.ingress_capacity = resources.ingress_capacity;
            value.rss_bytes = resources.rss_bytes;
            value.open_files = resources.open_files;
            value.journal_bytes = resources.journal_bytes;
            value.tick_duration_ns = resources.tick_duration_ns;
            assert_eq!(supervisor.evaluate(value).expect("degraded").reason, reason);
        }
    }

    #[test]
    fn watchdog_exact_boundary_passes_then_halt_is_absorbing() {
        let mut supervisor = OperationalSupervisor::new(config()).expect("supervisor");
        supervisor.evaluate(sample(100, 0)).expect("ready");
        let mut boundary = sample(200, 1);
        boundary.last_progress_ns = Some(100);
        assert_eq!(
            supervisor.evaluate(boundary).expect("boundary").mode,
            OperationalMode::Ready
        );
        let mut expired = sample(201, 2);
        expired.last_progress_ns = Some(100);
        assert_eq!(
            supervisor.evaluate(expired),
            Err(OperationalError::Integrity(
                OperationalReason::WatchdogExpired
            ))
        );
        assert_eq!(supervisor.snapshot().mode, OperationalMode::Halted);
        assert!(matches!(
            supervisor.evaluate(sample(202, 3)),
            Err(OperationalError::Halted(_))
        ));
    }

    #[test]
    fn clock_sequence_queue_progress_and_runtime_integrity_halt() {
        let mut clock = OperationalSupervisor::new(config()).expect("clock");
        clock.evaluate(sample(100, 1)).expect("first");
        assert_eq!(
            clock.evaluate(sample(99, 2)),
            Err(OperationalError::Integrity(
                OperationalReason::ClockRegression
            ))
        );

        let mut sequence = OperationalSupervisor::new(config()).expect("sequence");
        sequence.evaluate(sample(100, 2)).expect("first");
        assert_eq!(
            sequence.evaluate(sample(101, 1)),
            Err(OperationalError::Integrity(
                OperationalReason::SequenceRegression
            ))
        );

        let mut queue = OperationalSupervisor::new(config()).expect("queue");
        let mut impossible = sample(100, 0);
        impossible.ingress_depth = 11;
        assert_eq!(
            queue.evaluate(impossible),
            Err(OperationalError::Integrity(
                OperationalReason::ImpossibleIngressDepth
            ))
        );

        let mut future = OperationalSupervisor::new(config()).expect("future");
        let mut future_sample = sample(100, 0);
        future_sample.last_progress_ns = Some(101);
        assert_eq!(
            future.evaluate(future_sample),
            Err(OperationalError::Integrity(
                OperationalReason::FutureProgress
            ))
        );

        let mut runtime = OperationalSupervisor::new(config()).expect("runtime");
        let mut halted = sample(100, 0);
        halted.runtime_mode = RuntimeMode::Halted;
        assert_eq!(
            runtime.evaluate(halted),
            Err(OperationalError::Integrity(
                OperationalReason::RuntimeHalted
            ))
        );
    }

    #[test]
    fn drain_blocks_ready_and_stop_requires_drain() {
        let mut supervisor = OperationalSupervisor::new(config()).expect("supervisor");
        assert_eq!(
            supervisor.mark_stopped(),
            Err(OperationalError::NotDraining)
        );
        supervisor.evaluate(sample(100, 0)).expect("ready");
        assert_eq!(
            supervisor.begin_drain().expect("drain").mode,
            OperationalMode::Draining
        );
        assert_eq!(
            supervisor
                .evaluate(sample(101, 1))
                .expect("still drain")
                .mode,
            OperationalMode::Draining
        );
        assert_eq!(
            supervisor.mark_stopped().expect("stop").mode,
            OperationalMode::Stopped
        );
        assert_eq!(supervisor.begin_drain(), Err(OperationalError::Stopped));
    }

    #[test]
    fn openmetrics_and_digest_are_stable_and_identifier_free() {
        let mut first = OperationalSupervisor::new(config()).expect("first");
        let mut second = OperationalSupervisor::new(config()).expect("second");
        let left = first.evaluate(sample(100, 0)).expect("left");
        let right = second.evaluate(sample(100, 0)).expect("right");
        assert_eq!(left.digest, right.digest);
        let metrics = render_openmetrics(&left);
        assert!(metrics.contains("poly_shadow_ready 1"));
        assert!(metrics.contains("poly_shadow_reason_code 1"));
        assert!(metrics.ends_with("# EOF\n"));
        assert!(!metrics.contains("market_id"));
        assert_eq!(metrics, render_openmetrics(&right));
    }

    #[test]
    fn stress_profiles_are_bounded_and_seven_day_completes() {
        let smoke = run_stress_profile(StressProfile::Smoke).expect("smoke");
        let smoke_again = run_stress_profile(StressProfile::Smoke).expect("smoke again");
        assert_eq!(smoke, smoke_again);
        assert_eq!(smoke.sessions, 6);
        assert_eq!(smoke.finalized_sessions, 6);
        assert!(smoke.encoded_bytes > 0);
        assert!(smoke.syncs > smoke.records);

        let seven = run_stress_profile(StressProfile::SevenDay).expect("seven day");
        assert_eq!(seven.ticks, 169);
        assert_eq!(seven.sessions, 336);
        assert_eq!(seven.finalized_sessions, 336);
        assert_eq!(seven.ready_observations, 336);
        assert_eq!(seven.records, 505);
        assert!(seven.encoded_bytes > smoke.encoded_bytes);
    }
}
