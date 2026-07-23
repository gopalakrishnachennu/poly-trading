#![forbid(unsafe_code)]

//! Deterministic offline conditional-token transaction simulation.
//!
//! No type in this crate can sign, authenticate, call RPC, access a wallet,
//! submit a transaction, or retry automatically.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, CtfRecovery,
    CtfRuntimeCheckpoint, DurableCtfRuntime, StorageError,
};

use accounting_ledger::{
    CommandId as LedgerCommandId, LedgerCommand, LockId, LockStatus, ReservationId, TokenKey,
};
use paired_settlement_runtime::{
    PairedSettlementCommand, PairedSettlementOutcome, PairedSettlementRuntime,
};
use serde::{Deserialize, Serialize};
use settlement_reconciliation::ReconcilerConfig;
use std::collections::BTreeMap;
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CtfCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ConversionId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ConversionRequest {
    Split {
        up: TokenKey,
        down: TokenKey,
        quantity_micros: i128,
    },
    Merge {
        lock_id: LockId,
        up: TokenKey,
        down: TokenKey,
        quantity_micros: i128,
    },
    Redemption {
        token: TokenKey,
        quantity_micros: i128,
        payout_micros: i128,
        resolution_digest: [u8; 32],
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ConversionState {
    Requested,
    Pending,
    Retrying { reason: String },
    Confirmed { transaction_hash: String },
    Failed { reason: String },
}

impl ConversionState {
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Confirmed { .. } | Self::Failed { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ConversionEvent {
    Pending { external_transaction_id: String },
    Retrying { reason: String },
    Confirmed { transaction_hash: String },
    Failed { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionObservation {
    pub conversion_id: ConversionId,
    pub source_sequence: u64,
    pub event: ConversionEvent,
    pub event_time_ns: i64,
    pub received_time_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionRecord {
    pub conversion_id: ConversionId,
    pub request: ConversionRequest,
    pub reservation_id: Option<ReservationId>,
    pub state: ConversionState,
    pub external_transaction_id: Option<String>,
    pub last_source_sequence: Option<u64>,
    pub last_event_time_ns: Option<i64>,
    pub last_received_time_ns: Option<i64>,
    pub accounting_posted: bool,
    pub requested_at_ns: i64,
    pub record_digest: [u8; 32],
}

impl ConversionRecord {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.record_digest == record_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CtfCommand {
    Parent {
        command_id: CtfCommandId,
        command: Box<PairedSettlementCommand>,
        recorded_at_ns: i64,
    },
    Request {
        command_id: CtfCommandId,
        conversion_id: ConversionId,
        request: ConversionRequest,
        recorded_at_ns: i64,
    },
    Observe {
        command_id: CtfCommandId,
        observation: ConversionObservation,
        recorded_at_ns: i64,
    },
}

impl CtfCommand {
    #[must_use]
    pub const fn command_id(&self) -> CtfCommandId {
        match self {
            Self::Parent { command_id, .. }
            | Self::Request { command_id, .. }
            | Self::Observe { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Parent { recorded_at_ns, .. }
            | Self::Request { recorded_at_ns, .. }
            | Self::Observe { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CtfStage {
    Parent,
    Request,
    Lifecycle,
    Accounting,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CtfDetail {
    Parent(Box<PairedSettlementOutcome>),
    Requested,
    Pending,
    Retrying,
    Confirmed,
    Failed,
    DuplicateSubmission,
    DuplicateTerminal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CtfOutcome {
    pub command_id: CtfCommandId,
    pub conversion_id: Option<ConversionId>,
    pub stage: CtfStage,
    pub detail: CtfDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CtfSnapshot {
    pub accepted_commands: u64,
    pub parent_digest: [u8; 32],
    pub transaction_count: usize,
    pub requested_count: usize,
    pub pending_count: usize,
    pub retrying_count: usize,
    pub confirmed_count: usize,
    pub failed_count: usize,
    pub accounting_posted_count: usize,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("CTF runtime configuration is invalid")]
    Config,
    #[error("CTF command timestamp is invalid or regressed")]
    Timestamp,
    #[error("CTF command exceeds its canonical bound")]
    CommandBound,
    #[error("CTF command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported CTF command version: {0}")]
    Version(u16),
    #[error("CTF command id was reused for different content")]
    IdempotencyConflict,
    #[error("conversion identity already exists or was substituted")]
    Identity,
    #[error("conversion request is invalid or lacks reconciled backing")]
    Request,
    #[error("conversion lifecycle transition is invalid")]
    Lifecycle,
    #[error("external transaction identity was reused or changed")]
    ExternalIdentity,
    #[error("conversion source history regressed or equivocated")]
    SourceHistory,
    #[error("conversion accounting failed: {0}")]
    Accounting(String),
    #[error("paired settlement child failed: {0}")]
    Parent(String),
    #[error("CTF arithmetic or counter overflow")]
    Overflow,
    #[error("CTF runtime is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct CtfTransactionRuntime {
    config: ReconcilerConfig,
    parent: PairedSettlementRuntime,
    records: BTreeMap<ConversionId, ConversionRecord>,
    external_ids: BTreeMap<String, ConversionId>,
    processed: BTreeMap<CtfCommandId, ([u8; 32], CtfOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl CtfTransactionRuntime {
    /// Creates an empty offline CTF transaction owner.
    ///
    /// # Errors
    ///
    /// Rejects an invalid reconciliation configuration.
    pub fn new(config: ReconcilerConfig) -> Result<Self, Error> {
        let parent = PairedSettlementRuntime::new(config.clone()).map_err(|_| Error::Config)?;
        Ok(Self {
            config,
            parent,
            records: BTreeMap::new(),
            external_ids: BTreeMap::new(),
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one transactionally composed command.
    ///
    /// # Errors
    ///
    /// Returns absorbing identity, history, lifecycle, accounting, child, or
    /// durability failures.
    pub fn apply(&mut self, command: &CtfCommand) -> Result<CtfOutcome, Error> {
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
        let mut outcome = match next.apply_fresh(command) {
            Ok(value) => value,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = next
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        outcome.outcome_digest = outcome_digest(&outcome);
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.processed.insert(id, (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    fn apply_fresh(&mut self, command: &CtfCommand) -> Result<CtfOutcome, Error> {
        match command {
            CtfCommand::Parent {
                command_id,
                command,
                recorded_at_ns,
            } => self.apply_parent(*command_id, command, *recorded_at_ns),
            CtfCommand::Request {
                command_id,
                conversion_id,
                request,
                recorded_at_ns,
            } => self.request(*command_id, *conversion_id, request, *recorded_at_ns),
            CtfCommand::Observe {
                command_id,
                observation,
                recorded_at_ns,
            } => self.observe(*command_id, observation, *recorded_at_ns),
        }
    }

    fn apply_parent(
        &mut self,
        id: CtfCommandId,
        command: &PairedSettlementCommand,
        at: i64,
    ) -> Result<CtfOutcome, Error> {
        if command.recorded_at_ns() != at {
            return Err(Error::Timestamp);
        }
        let result = self
            .parent
            .apply(command)
            .map_err(|error| Error::Parent(error.to_string()))?;
        Ok(outcome(
            id,
            None,
            CtfStage::Parent,
            CtfDetail::Parent(Box::new(result)),
        ))
    }

    fn request(
        &mut self,
        id: CtfCommandId,
        conversion_id: ConversionId,
        request: &ConversionRequest,
        at: i64,
    ) -> Result<CtfOutcome, Error> {
        if self.records.contains_key(&conversion_id) {
            return Err(Error::Identity);
        }
        if !self.parent.reconciliation_is_current() || !valid_request(request) {
            return Err(Error::Request);
        }
        let reservation_id = match request {
            ConversionRequest::Split {
                quantity_micros, ..
            } => {
                let reservation = derived_reservation_id(conversion_id);
                self.parent
                    .conversion_apply_batch(&[LedgerCommand::ReserveCollateral {
                        command_id: derived_ledger_id(conversion_id, b"reserve"),
                        reservation_id: reservation,
                        amount_micros: *quantity_micros,
                        recorded_at_ns: at,
                    }])
                    .map_err(|error| Error::Accounting(error.to_string()))?;
                Some(reservation)
            }
            ConversionRequest::Redemption {
                token,
                quantity_micros,
                ..
            } => {
                let reservation = derived_reservation_id(conversion_id);
                self.parent
                    .conversion_apply_batch(&[LedgerCommand::ReserveToken {
                        command_id: derived_ledger_id(conversion_id, b"reserve"),
                        reservation_id: reservation,
                        token: token.clone(),
                        quantity_micros: *quantity_micros,
                        recorded_at_ns: at,
                    }])
                    .map_err(|error| Error::Accounting(error.to_string()))?;
                Some(reservation)
            }
            ConversionRequest::Merge {
                lock_id,
                up,
                down,
                quantity_micros,
            } => {
                if self.records.values().any(|record| {
                    !record.state.is_terminal()
                        && matches!(record.request, ConversionRequest::Merge { lock_id: existing, .. } if existing == *lock_id)
                }) {
                    return Err(Error::Request);
                }
                if let Some(lock) = self.parent.conversion_pair_lock(*lock_id) {
                    if lock.status != LockStatus::Active
                        || lock.up != *up
                        || lock.down != *down
                        || lock.quantity_micros != *quantity_micros
                    {
                        return Err(Error::Request);
                    }
                } else {
                    self.parent
                        .conversion_apply_batch(&[LedgerCommand::LockPair {
                            command_id: derived_ledger_id(conversion_id, b"lock"),
                            lock_id: *lock_id,
                            up: up.clone(),
                            down: down.clone(),
                            quantity_micros: *quantity_micros,
                            recorded_at_ns: at,
                        }])
                        .map_err(|error| Error::Accounting(error.to_string()))?;
                }
                None
            }
        };
        let mut record = ConversionRecord {
            conversion_id,
            request: request.clone(),
            reservation_id,
            state: ConversionState::Requested,
            external_transaction_id: None,
            last_source_sequence: None,
            last_event_time_ns: None,
            last_received_time_ns: None,
            accounting_posted: false,
            requested_at_ns: at,
            record_digest: [0; 32],
        };
        record.record_digest = record_digest(&record);
        self.records.insert(conversion_id, record);
        Ok(outcome(
            id,
            Some(conversion_id),
            CtfStage::Request,
            CtfDetail::Requested,
        ))
    }

    fn observe(
        &mut self,
        id: CtfCommandId,
        observation: &ConversionObservation,
        at: i64,
    ) -> Result<CtfOutcome, Error> {
        validate_observation(observation, at)?;
        let current = self
            .records
            .get(&observation.conversion_id)
            .cloned()
            .ok_or(Error::Identity)?;
        validate_source(&current, observation)?;
        let (state, detail, accounting_posted) = self.transition(&current, observation, at)?;
        let record = self
            .records
            .get_mut(&observation.conversion_id)
            .ok_or(Error::Identity)?;
        record.state = state;
        record.last_source_sequence = Some(observation.source_sequence);
        record.last_event_time_ns = Some(observation.event_time_ns);
        record.last_received_time_ns = Some(observation.received_time_ns);
        record.accounting_posted = accounting_posted;
        if let ConversionEvent::Pending {
            external_transaction_id,
        } = &observation.event
        {
            if record.external_transaction_id.is_none() {
                record.external_transaction_id = Some(external_transaction_id.clone());
            }
        }
        record.record_digest = [0; 32];
        record.record_digest = record_digest(record);
        Ok(outcome(
            id,
            Some(observation.conversion_id),
            if accounting_posted {
                CtfStage::Accounting
            } else {
                CtfStage::Lifecycle
            },
            detail,
        ))
    }

    fn transition(
        &mut self,
        current: &ConversionRecord,
        observation: &ConversionObservation,
        at: i64,
    ) -> Result<(ConversionState, CtfDetail, bool), Error> {
        if let Some(detail) = duplicate_terminal(current, &observation.event) {
            return Ok((current.state.clone(), detail, current.accounting_posted));
        }
        if current.state.is_terminal() {
            return Err(Error::Lifecycle);
        }
        match &observation.event {
            ConversionEvent::Pending {
                external_transaction_id,
            } => {
                validate_text(external_transaction_id)?;
                if let Some(existing) = &current.external_transaction_id {
                    if existing != external_transaction_id {
                        return Err(Error::ExternalIdentity);
                    }
                    return Ok((
                        current.state.clone(),
                        CtfDetail::DuplicateSubmission,
                        current.accounting_posted,
                    ));
                }
                if let Some(owner) = self.external_ids.get(external_transaction_id) {
                    if *owner != current.conversion_id {
                        return Err(Error::ExternalIdentity);
                    }
                }
                self.external_ids
                    .insert(external_transaction_id.clone(), current.conversion_id);
                Ok((ConversionState::Pending, CtfDetail::Pending, false))
            }
            ConversionEvent::Retrying { reason } => {
                validate_text(reason)?;
                if current.external_transaction_id.is_none()
                    || !matches!(
                        current.state,
                        ConversionState::Pending | ConversionState::Retrying { .. }
                    )
                {
                    return Err(Error::Lifecycle);
                }
                Ok((
                    ConversionState::Retrying {
                        reason: reason.clone(),
                    },
                    CtfDetail::Retrying,
                    false,
                ))
            }
            ConversionEvent::Confirmed { transaction_hash } => {
                validate_text(transaction_hash)?;
                if current.external_transaction_id.is_none()
                    || !matches!(
                        current.state,
                        ConversionState::Pending | ConversionState::Retrying { .. }
                    )
                {
                    return Err(Error::Lifecycle);
                }
                let command = confirmed_command(current, transaction_hash, at)?;
                self.parent
                    .conversion_apply_batch(&[command])
                    .map_err(|error| Error::Accounting(error.to_string()))?;
                Ok((
                    ConversionState::Confirmed {
                        transaction_hash: transaction_hash.clone(),
                    },
                    CtfDetail::Confirmed,
                    true,
                ))
            }
            ConversionEvent::Failed { reason } => {
                validate_text(reason)?;
                if let Some(reservation_id) = current.reservation_id {
                    self.parent
                        .conversion_apply_batch(&[LedgerCommand::ReleaseReservation {
                            command_id: derived_ledger_id(current.conversion_id, b"release"),
                            reservation_id,
                            recorded_at_ns: at,
                        }])
                        .map_err(|error| Error::Accounting(error.to_string()))?;
                }
                Ok((
                    ConversionState::Failed {
                        reason: reason.clone(),
                    },
                    CtfDetail::Failed,
                    false,
                ))
            }
        }
    }

    #[must_use]
    pub const fn parent(&self) -> &PairedSettlementRuntime {
        &self.parent
    }

    #[must_use]
    pub fn record(&self, id: ConversionId) -> Option<&ConversionRecord> {
        self.records.get(&id)
    }

    #[must_use]
    pub fn snapshot(&self) -> CtfSnapshot {
        let count = |predicate: fn(&ConversionState) -> bool| {
            self.records
                .values()
                .filter(|record| predicate(&record.state))
                .count()
        };
        CtfSnapshot {
            accepted_commands: self.accepted_commands,
            parent_digest: self.parent.snapshot().digest,
            transaction_count: self.records.len(),
            requested_count: count(|state| matches!(state, ConversionState::Requested)),
            pending_count: count(|state| matches!(state, ConversionState::Pending)),
            retrying_count: count(|state| matches!(state, ConversionState::Retrying { .. })),
            confirmed_count: count(|state| matches!(state, ConversionState::Confirmed { .. })),
            failed_count: count(|state| matches!(state, ConversionState::Failed { .. })),
            accounting_posted_count: self
                .records
                .values()
                .filter(|record| record.accounting_posted)
                .count(),
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
        hasher.update(b"ctf-transaction-runtime-state-v1");
        hash_json(&mut hasher, &self.config.chain_id);
        hash_json(&mut hasher, &self.config.wallet);
        hash_json(&mut hasher, &self.config.confirmation_grace_ns);
        hash_json(&mut hasher, &self.config.max_intents);
        hash_json(&mut hasher, &self.config.max_tokens);
        hasher.update(&self.parent.snapshot().digest);
        for (id, record) in &self.records {
            hasher.update(&id.0);
            hash_json(&mut hasher, record);
        }
        for (external, owner) in &self.external_ids {
            hash_json(&mut hasher, external);
            hasher.update(&owner.0);
        }
        for (id, (content, result)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_json(&mut hasher, result);
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

fn valid_request(request: &ConversionRequest) -> bool {
    match request {
        ConversionRequest::Split {
            up,
            down,
            quantity_micros,
        }
        | ConversionRequest::Merge {
            up,
            down,
            quantity_micros,
            ..
        } => {
            *quantity_micros > 0
                && up != down
                && up.condition_id == down.condition_id
                && valid_token(up)
                && valid_token(down)
        }
        ConversionRequest::Redemption {
            token,
            quantity_micros,
            payout_micros,
            resolution_digest,
        } => {
            valid_token(token)
                && *quantity_micros > 0
                && *payout_micros >= 0
                && payout_micros <= quantity_micros
                && *resolution_digest != [0; 32]
        }
    }
}

fn valid_token(token: &TokenKey) -> bool {
    !token.condition_id.is_empty()
        && token.condition_id.len() <= 256
        && !token.token_id.is_empty()
        && token.token_id.len() <= 256
}

fn validate_observation(observation: &ConversionObservation, at: i64) -> Result<(), Error> {
    if observation.source_sequence == 0
        || observation.event_time_ns < 0
        || observation.received_time_ns < observation.event_time_ns
        || at < observation.received_time_ns
    {
        return Err(Error::Timestamp);
    }
    Ok(())
}

fn validate_source(
    record: &ConversionRecord,
    observation: &ConversionObservation,
) -> Result<(), Error> {
    if record
        .last_source_sequence
        .is_some_and(|value| observation.source_sequence <= value)
        || record
            .last_event_time_ns
            .is_some_and(|value| observation.event_time_ns < value)
        || record
            .last_received_time_ns
            .is_some_and(|value| observation.received_time_ns < value)
    {
        return Err(Error::SourceHistory);
    }
    Ok(())
}

fn confirmed_command(
    record: &ConversionRecord,
    confirmation: &str,
    at: i64,
) -> Result<LedgerCommand, Error> {
    let command_id = derived_ledger_id(record.conversion_id, b"confirm");
    match &record.request {
        ConversionRequest::Split {
            up,
            down,
            quantity_micros,
        } => Ok(LedgerCommand::ConfirmSplit {
            command_id,
            reservation_id: record.reservation_id.ok_or(Error::Request)?,
            up: up.clone(),
            down: down.clone(),
            quantity_micros: *quantity_micros,
            confirmation: confirmation.to_owned(),
            recorded_at_ns: at,
        }),
        ConversionRequest::Merge { lock_id, .. } => Ok(LedgerCommand::ConfirmMerge {
            command_id,
            lock_id: *lock_id,
            confirmation: confirmation.to_owned(),
            recorded_at_ns: at,
        }),
        ConversionRequest::Redemption {
            quantity_micros,
            payout_micros,
            ..
        } => Ok(LedgerCommand::ConfirmRedemption {
            command_id,
            reservation_id: record.reservation_id.ok_or(Error::Request)?,
            quantity_micros: *quantity_micros,
            payout_micros: *payout_micros,
            confirmation: confirmation.to_owned(),
            recorded_at_ns: at,
        }),
    }
}

fn duplicate_terminal(record: &ConversionRecord, event: &ConversionEvent) -> Option<CtfDetail> {
    match (&record.state, event) {
        (
            ConversionState::Confirmed {
                transaction_hash: existing,
            },
            ConversionEvent::Confirmed { transaction_hash },
        ) if existing == transaction_hash => Some(CtfDetail::DuplicateTerminal),
        (ConversionState::Failed { reason: existing }, ConversionEvent::Failed { reason })
            if existing == reason =>
        {
            Some(CtfDetail::DuplicateTerminal)
        }
        _ => None,
    }
}

fn derived_reservation_id(id: ConversionId) -> ReservationId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ctf-conversion-reservation-v1");
    hasher.update(&id.0);
    ReservationId(*hasher.finalize().as_bytes())
}

fn derived_ledger_id(id: ConversionId, domain: &[u8]) -> LedgerCommandId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ctf-conversion-ledger-v1");
    hasher.update(domain);
    hasher.update(&id.0);
    LedgerCommandId(*hasher.finalize().as_bytes())
}

fn validate_text(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES {
        Err(Error::Identity)
    } else {
        Ok(())
    }
}

fn record_digest(record: &ConversionRecord) -> [u8; 32] {
    let mut copy = record.clone();
    copy.record_digest = [0; 32];
    *blake3::hash(&serde_json::to_vec(&copy).expect("serializable record")).as_bytes()
}

fn outcome(
    id: CtfCommandId,
    conversion_id: Option<ConversionId>,
    stage: CtfStage,
    detail: CtfDetail,
) -> CtfOutcome {
    CtfOutcome {
        command_id: id,
        conversion_id,
        stage,
        detail,
        outcome_digest: [0; 32],
    }
}

fn outcome_digest(outcome: &CtfOutcome) -> [u8; 32] {
    let mut copy = outcome.clone();
    copy.outcome_digest = [0; 32];
    *blake3::hash(&serde_json::to_vec(&copy).expect("serializable outcome")).as_bytes()
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: CtfCommand,
}

pub(crate) fn encode_command(command: &CtfCommand) -> Result<Vec<u8>, Error> {
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

pub(crate) fn decode_command(bytes: &[u8]) -> Result<CtfCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let wire: WireCommand =
        serde_json::from_slice(bytes).map_err(|error| Error::Json(error.to_string()))?;
    if wire.version != WIRE_VERSION {
        return Err(Error::Version(wire.version));
    }
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
