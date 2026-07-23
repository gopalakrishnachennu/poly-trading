#![forbid(unsafe_code)]

//! Bounded single-writer runtime for authoritative public market state.

use event_schema::{EventEnvelope, EventSource};
use order_book_replay::{EpochStatus, ReplayState};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{interval, MissedTickBehavior};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorMode {
    Starting,
    CollectingSnapshots,
    RecoveringSnapshot,
    Ready,
    Stale,
    Inactive,
    Shutdown,
    Closed,
    Halted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActorSnapshot {
    pub mode: ActorMode,
    pub ready: bool,
    pub epoch: u64,
    pub last_sequence: Option<u64>,
    pub book_count: usize,
    pub digest: [u8; 32],
    pub last_market_event_ns: Option<i64>,
    pub last_market_received_ns: Option<i64>,
    pub halt_reason: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct ActorConfig {
    pub channel_capacity: usize,
    pub stale_after: Duration,
    pub health_interval: Duration,
}

impl Default for ActorConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 4_096,
            stale_after: Duration::from_secs(5),
            health_interval: Duration::from_millis(250),
        }
    }
}

#[derive(Debug, Error)]
pub enum ActorError {
    #[error("actor channel capacity and health durations must be positive")]
    InvalidConfig,
    #[error("actor is permanently halted: {0}")]
    Halted(String),
    #[error("authoritative transition failed: {0}")]
    Transition(String),
    #[error("market receive time regressed from {previous} to {actual}")]
    ReceiveTimeRegression { previous: i64, actual: i64 },
    #[error("health clock {now} is before last market receive time {last_received}")]
    HealthClockRegression { now: i64, last_received: i64 },
    #[error("system clock is before the Unix epoch")]
    ClockBeforeEpoch,
    #[error("system clock timestamp overflow")]
    ClockOverflow,
}

#[derive(Clone, Debug)]
pub struct LiveStateCore {
    replay: ReplayState,
    stale_after_ns: i64,
    mode: ActorMode,
    last_market_event_ns: Option<i64>,
    last_market_received_ns: Option<i64>,
    halt_reason: Option<String>,
}

impl LiveStateCore {
    /// Creates a deterministic health core with an explicit staleness budget.
    ///
    /// # Errors
    ///
    /// Returns [`ActorError::InvalidConfig`] for zero or unrepresentable
    /// durations.
    pub fn new(stale_after: Duration) -> Result<Self, ActorError> {
        let stale_after_ns =
            i64::try_from(stale_after.as_nanos()).map_err(|_| ActorError::InvalidConfig)?;
        if stale_after_ns == 0 {
            return Err(ActorError::InvalidConfig);
        }
        Ok(Self {
            replay: ReplayState::default(),
            stale_after_ns,
            mode: ActorMode::Starting,
            last_market_event_ns: None,
            last_market_received_ns: None,
            halt_reason: None,
        })
    }

    /// Applies one journal-equivalent envelope or permanently halts.
    ///
    /// # Errors
    ///
    /// Returns [`ActorError`] when already halted, receive time regresses, or
    /// the shared replay transition rejects the envelope.
    pub fn apply(&mut self, envelope: &EventEnvelope) -> Result<(), ActorError> {
        if let Some(reason) = &self.halt_reason {
            return Err(ActorError::Halted(reason.clone()));
        }
        if envelope.source == EventSource::Market {
            if envelope.event_time_ns < 0 {
                let error = ActorError::Transition("market event time is unavailable".to_owned());
                self.halt(error.to_string());
                return Err(error);
            }
            if let Some(previous) = self.last_market_received_ns {
                if envelope.received_time_ns < previous {
                    let error = ActorError::ReceiveTimeRegression {
                        previous,
                        actual: envelope.received_time_ns,
                    };
                    self.halt(error.to_string());
                    return Err(error);
                }
            }
        }

        if let Err(error) = self.replay.apply(envelope) {
            let message = error.to_string();
            self.halt(message.clone());
            return Err(ActorError::Transition(message));
        }
        if envelope.source == EventSource::Market {
            self.last_market_event_ns = Some(envelope.event_time_ns);
            self.last_market_received_ns = Some(envelope.received_time_ns);
        }
        self.refresh_mode(envelope.received_time_ns)?;
        Ok(())
    }

    /// Re-evaluates freshness using caller-supplied time.
    ///
    /// # Errors
    ///
    /// Returns [`ActorError`] and permanently halts when `now_ns` is before the
    /// last market receive time.
    pub fn evaluate_health(&mut self, now_ns: i64) -> Result<(), ActorError> {
        if let Some(reason) = &self.halt_reason {
            return Err(ActorError::Halted(reason.clone()));
        }
        self.refresh_mode(now_ns)
    }

    pub fn close(&mut self) {
        if self.halt_reason.is_none() && self.replay.status() != EpochStatus::Shutdown {
            self.mode = ActorMode::Closed;
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ActorSnapshot {
        ActorSnapshot {
            mode: self.mode,
            ready: self.mode == ActorMode::Ready,
            epoch: self.replay.epoch(),
            last_sequence: self.replay.last_sequence(),
            book_count: self.replay.books().len(),
            digest: self.replay.digest(),
            last_market_event_ns: self.last_market_event_ns,
            last_market_received_ns: self.last_market_received_ns,
            halt_reason: self.halt_reason.clone(),
        }
    }

    #[must_use]
    pub const fn replay(&self) -> &ReplayState {
        &self.replay
    }

    fn refresh_mode(&mut self, now_ns: i64) -> Result<(), ActorError> {
        self.mode = match self.replay.status() {
            EpochStatus::Inactive => ActorMode::Inactive,
            EpochStatus::CollectingSnapshots => ActorMode::CollectingSnapshots,
            EpochStatus::Shutdown => ActorMode::Shutdown,
            EpochStatus::Synchronized => {
                if !self.replay.is_authoritative() {
                    self.mode = ActorMode::RecoveringSnapshot;
                    return Ok(());
                }
                let Some(last_received) = self.last_market_received_ns else {
                    self.halt("synchronized epoch has no market receive time".to_owned());
                    return Err(ActorError::Transition(
                        "synchronized epoch has no market receive time".to_owned(),
                    ));
                };
                let age = if now_ns < last_received {
                    Err(ActorError::HealthClockRegression {
                        now: now_ns,
                        last_received,
                    })
                } else {
                    now_ns
                        .checked_sub(last_received)
                        .ok_or(ActorError::HealthClockRegression {
                            now: now_ns,
                            last_received,
                        })
                };
                match age {
                    Ok(age) if age <= self.stale_after_ns => ActorMode::Ready,
                    Ok(_) => ActorMode::Stale,
                    Err(error) => {
                        self.halt(error.to_string());
                        return Err(error);
                    }
                }
            }
        };
        Ok(())
    }

    fn halt(&mut self, reason: String) {
        self.mode = ActorMode::Halted;
        self.halt_reason = Some(reason);
    }
}

#[derive(Debug)]
pub struct ActorRuntime {
    pub sender: mpsc::Sender<EventEnvelope>,
    pub snapshots: watch::Receiver<ActorSnapshot>,
    pub task: JoinHandle<ActorSnapshot>,
}

/// Spawns the one-writer Tokio wrapper around [`LiveStateCore`].
///
/// # Errors
///
/// Returns [`ActorError::InvalidConfig`] for zero channel capacity or invalid
/// timing configuration.
pub fn spawn_actor(config: ActorConfig) -> Result<ActorRuntime, ActorError> {
    if config.channel_capacity == 0 || config.health_interval.is_zero() {
        return Err(ActorError::InvalidConfig);
    }
    let core = LiveStateCore::new(config.stale_after)?;
    let initial = core.snapshot();
    let (sender, mut receiver) = mpsc::channel(config.channel_capacity);
    let (snapshot_sender, snapshots) = watch::channel(initial);
    let task = tokio::spawn(async move {
        let mut core = core;
        let mut health = interval(config.health_interval);
        health.set_missed_tick_behavior(MissedTickBehavior::Delay);
        health.tick().await;
        loop {
            tokio::select! {
                event = receiver.recv() => {
                    let Some(event) = event else {
                        core.close();
                        let snapshot = core.snapshot();
                        snapshot_sender.send_replace(snapshot.clone());
                        return snapshot;
                    };
                    if core.apply(&event).is_err() {
                        let snapshot = core.snapshot();
                        snapshot_sender.send_replace(snapshot.clone());
                        return snapshot;
                    }
                    snapshot_sender.send_replace(core.snapshot());
                }
                _ = health.tick() => {
                    match now_ns().and_then(|now| core.evaluate_health(now)) {
                        Ok(()) => {
                            snapshot_sender.send_replace(core.snapshot());
                        }
                        Err(error) => {
                            if core.halt_reason.is_none() {
                                core.halt(error.to_string());
                            }
                            let snapshot = core.snapshot();
                            snapshot_sender.send_replace(snapshot.clone());
                            return snapshot;
                        }
                    }
                }
            }
        }
    });
    Ok(ActorRuntime {
        sender,
        snapshots,
        task,
    })
}

fn now_ns() -> Result<i64, ActorError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ActorError::ClockBeforeEpoch)?;
    i64::try_from(duration.as_nanos()).map_err(|_| ActorError::ClockOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use order_book_replay::ReplayState;
    use serde_json::{json, Value};

    const SYSTEM_ID: &str = "__public_market_gateway__";
    const START: &[u8] = b"PUBLIC_MARKET_EPOCH_START_V1";
    const SYNCED: &[u8] = b"PUBLIC_MARKET_EPOCH_SYNCED_V1";
    const SHUTDOWN: &[u8] = b"PUBLIC_MARKET_EPOCH_SHUTDOWN_V1";
    const ASSET: &str = "11";

    fn condition() -> String {
        format!("0x{}", "a".repeat(64))
    }

    fn system(sequence: u64, received_time_ns: i64, payload: &[u8]) -> EventEnvelope {
        EventEnvelope::new(
            EventSource::System,
            sequence,
            received_time_ns,
            received_time_ns,
            SYSTEM_ID.to_owned(),
            payload.to_vec(),
        )
        .expect("system")
    }

    fn market(sequence: u64, received_time_ns: i64, kind: u8, value: &Value) -> EventEnvelope {
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .expect("timestamp")
            .parse::<i64>()
            .expect("timestamp number");
        let json = serde_json::to_vec(value).expect("json");
        let mut payload = Vec::new();
        payload.extend_from_slice(&1_u16.to_le_bytes());
        payload.push(kind);
        payload.push(0);
        payload.extend_from_slice(&timestamp.to_le_bytes());
        payload.extend_from_slice(&1_u16.to_le_bytes());
        payload.extend_from_slice(
            &u16::try_from(ASSET.len())
                .expect("asset length")
                .to_le_bytes(),
        );
        payload.extend_from_slice(ASSET.as_bytes());
        payload.extend_from_slice(
            &u32::try_from(json.len())
                .expect("json length")
                .to_le_bytes(),
        );
        payload.extend_from_slice(&json);
        EventEnvelope::new(
            EventSource::Market,
            sequence,
            timestamp * 1_000_000,
            received_time_ns,
            condition(),
            payload,
        )
        .expect("market")
    }

    fn book(sequence: u64, received_time_ns: i64) -> EventEnvelope {
        market(
            sequence,
            received_time_ns,
            1,
            &json!({
                "event_type": "book",
                "market": condition(),
                "asset_id": ASSET,
                "bids": [{"price": "0.4", "size": "2"}],
                "asks": [{"price": "0.6", "size": "3"}],
                "timestamp": "1"
            }),
        )
    }

    fn crossed_change(sequence: u64, received_time_ns: i64) -> EventEnvelope {
        market(
            sequence,
            received_time_ns,
            2,
            &json!({
                "event_type": "price_change",
                "market": condition(),
                "price_changes": [{
                    "asset_id": ASSET,
                    "side": "BUY",
                    "price": "0.70",
                    "size": "2"
                }],
                "timestamp": "1"
            }),
        )
    }

    #[test]
    fn readiness_transitions_are_explicit() {
        let mut core = LiveStateCore::new(Duration::from_nanos(100)).expect("core");
        assert_eq!(core.snapshot().mode, ActorMode::Starting);
        core.apply(&system(0, 1_000, START)).expect("start");
        assert_eq!(core.snapshot().mode, ActorMode::CollectingSnapshots);
        core.apply(&book(1, 1_100)).expect("book");
        assert!(!core.snapshot().ready);
        core.apply(&system(2, 1_150, SYNCED)).expect("synced");
        assert_eq!(core.snapshot().mode, ActorMode::Ready);
        core.evaluate_health(1_200).expect("fresh");
        assert!(core.snapshot().ready);
        core.apply(&crossed_change(3, 1_200))
            .expect("crossed delta");
        assert_eq!(core.snapshot().mode, ActorMode::RecoveringSnapshot);
        assert!(!core.snapshot().ready);
        core.apply(&book(4, 1_200)).expect("recovery snapshot");
        assert_eq!(core.snapshot().mode, ActorMode::Ready);
        core.evaluate_health(1_301).expect("stale");
        assert_eq!(core.snapshot().mode, ActorMode::Stale);
        assert!(!core.snapshot().ready);
        core.apply(&system(5, 1_302, SHUTDOWN)).expect("shutdown");
        assert_eq!(core.snapshot().mode, ActorMode::Shutdown);
    }

    #[test]
    fn transition_failure_halts_with_last_valid_digest() {
        let mut core = LiveStateCore::new(Duration::from_secs(1)).expect("core");
        core.apply(&system(0, 1_000, START)).expect("start");
        let valid_digest = core.snapshot().digest;
        assert!(matches!(
            core.apply(&book(2, 1_100)),
            Err(ActorError::Transition(_))
        ));
        let halted = core.snapshot();
        assert_eq!(halted.mode, ActorMode::Halted);
        assert!(!halted.ready);
        assert_eq!(halted.digest, valid_digest);
        assert!(matches!(
            core.apply(&book(1, 1_100)),
            Err(ActorError::Halted(_))
        ));
    }

    #[test]
    fn clock_regression_halts() {
        let mut core = LiveStateCore::new(Duration::from_secs(1)).expect("core");
        core.apply(&system(0, 1_000, START)).expect("start");
        core.apply(&book(1, 1_100)).expect("book");
        core.apply(&system(2, 1_150, SYNCED)).expect("synced");
        assert!(matches!(
            core.evaluate_health(1_099),
            Err(ActorError::HealthClockRegression { .. })
        ));
        assert_eq!(core.snapshot().mode, ActorMode::Halted);
    }

    #[tokio::test]
    async fn bounded_actor_matches_offline_replay() {
        let events = vec![
            system(0, 1_000, START),
            book(1, 1_100),
            system(2, 1_150, SYNCED),
            system(3, 1_200, SHUTDOWN),
        ];
        let mut offline = ReplayState::default();
        for event in &events {
            offline.apply(event).expect("offline");
        }

        let runtime = spawn_actor(ActorConfig {
            channel_capacity: 2,
            stale_after: Duration::from_secs(1),
            health_interval: Duration::from_secs(3_600),
        })
        .expect("runtime");
        for event in events {
            runtime.sender.send(event).await.expect("send");
        }
        drop(runtime.sender);
        let terminal = runtime.task.await.expect("join");
        assert_eq!(terminal.mode, ActorMode::Shutdown);
        assert_eq!(terminal.digest, offline.digest());
        assert_eq!(terminal.last_sequence, offline.last_sequence());
    }
}
