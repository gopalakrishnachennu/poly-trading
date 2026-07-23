#![forbid(unsafe_code)]

//! Deterministic proposal-only boundary between session truth and portfolio risk.
//!
//! This crate can produce an inert [`OrderExposure`]. It cannot evaluate
//! portfolio risk, reserve capital, sign, authenticate, connect, or submit.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableProposalEngine,
    ProposalCheckpoint, ProposalRecovery, StorageError,
};

use accounting_ledger::TokenKey;
use market_session::{
    coordination_frame_digest, CoordinationFrame, CoordinatorSnapshot, SessionKey, SessionPhase,
    TokenBookView,
};
use portfolio_risk::{OrderExposure, OrderSide, RiskOrderId};
use public_market_data::{Asset, MarketIdentity};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 256 * 1024;
const MAX_TEXT_BYTES: usize = 512;
const MAX_QUANTITY_MICROS: i128 = 1_000_000_000_000;
const MAX_FEE_MICROS: i128 = 1_000_000_000;
const MAX_CONTEXT_VALIDITY_NS: i64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ProposalCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ProposalId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyClass {
    CompleteSetArbitrage,
    MarketMaking,
    SequentialHedge,
    StatisticalDirectional,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyAsset {
    Bitcoin,
    Ethereum,
}

impl From<Asset> for StrategyAsset {
    fn from(value: Asset) -> Self {
        match value {
            Asset::Bitcoin => Self::Bitcoin,
            Asset::Ethereum => Self::Ethereum,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalLevel {
    pub price_micros: i64,
    pub quantity_micros: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalBook {
    pub authoritative: bool,
    pub best_bid: Option<ProposalLevel>,
    pub best_ask: Option<ProposalLevel>,
}

impl From<TokenBookView> for ProposalBook {
    fn from(value: TokenBookView) -> Self {
        Self {
            authoritative: value.authoritative,
            best_bid: value.best_bid.map(|(price, quantity)| ProposalLevel {
                price_micros: price.as_micros(),
                quantity_micros: quantity.as_micros(),
            }),
            best_ask: value.best_ask.map(|(price, quantity)| ProposalLevel {
                price_micros: price.as_micros(),
                quantity_micros: quantity.as_micros(),
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyContext {
    pub captured_at_ns: i64,
    pub valid_until_ns: i64,
    pub coordinator_evaluated_at_ns: i64,
    pub coordinator_digest: [u8; 32],
    pub applied_frame_digest: [u8; 32],
    pub session_digest: [u8; 32],
    pub market_digest: [u8; 32],
    pub reference_digest: [u8; 32],
    pub supervision_digest: [u8; 32],
    pub asset: StrategyAsset,
    pub session_start_time_ms: i64,
    pub session_end_time_ms: i64,
    pub condition_id: String,
    pub up_token: TokenKey,
    pub down_token: TokenKey,
    pub current: bool,
    pub active_ready: bool,
    pub up_book: Option<ProposalBook>,
    pub down_book: Option<ProposalBook>,
    pub context_digest: [u8; 32],
}

impl StrategyContext {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.context_digest == context_digest(self)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CaptureError {
    #[error("strategy context time is invalid")]
    Time,
    #[error("coordinator snapshot did not apply the supplied frame")]
    Provenance,
    #[error("session identity does not match coordinator state")]
    Identity,
    #[error("session source state is missing from the applied frame")]
    SourceMissing,
    #[error("strategy context identifier is invalid")]
    Identifier,
}

/// Captures an immutable proposal context from exact applied session truth.
///
/// A degraded context is valid audit input but cannot produce a candidate.
///
/// # Errors
///
/// Rejects time, applied-frame, identity, source, or identifier mismatch.
pub fn capture_context(
    coordinator: &CoordinatorSnapshot,
    frame: &CoordinationFrame,
    identity: &MarketIdentity,
    captured_at_ns: i64,
    valid_until_ns: i64,
) -> Result<StrategyContext, CaptureError> {
    let session_end_ns = identity
        .end_time_ms
        .checked_mul(1_000_000)
        .ok_or(CaptureError::Time)?;
    if captured_at_ns < 0
        || captured_at_ns != frame.now_ns
        || valid_until_ns < captured_at_ns
        || valid_until_ns > session_end_ns
        || valid_until_ns - captured_at_ns > MAX_CONTEXT_VALIDITY_NS
    {
        return Err(CaptureError::Time);
    }
    let applied = coordination_frame_digest(frame);
    if coordinator.evaluated_at_ns != Some(frame.now_ns)
        || coordinator.applied_frame_digest != Some(applied)
    {
        return Err(CaptureError::Provenance);
    }
    let key = SessionKey::from(identity);
    let session = coordinator
        .sessions
        .get(&key)
        .ok_or(CaptureError::Identity)?;
    if session.condition_id != identity.condition_id
        || session.end_time_ms != identity.end_time_ms
        || identity.condition_id.is_empty()
        || identity.up_token_id.is_empty()
        || identity.down_token_id.is_empty()
        || identity.condition_id.len() > MAX_TEXT_BYTES
        || identity.up_token_id.len() > MAX_TEXT_BYTES
        || identity.down_token_id.len() > MAX_TEXT_BYTES
    {
        return Err(CaptureError::Identifier);
    }
    let source = frame
        .sessions
        .get(&key)
        .ok_or(CaptureError::SourceMissing)?;
    let mut context = StrategyContext {
        captured_at_ns,
        valid_until_ns,
        coordinator_evaluated_at_ns: frame.now_ns,
        coordinator_digest: coordinator.digest,
        applied_frame_digest: applied,
        session_digest: session.digest,
        market_digest: frame.market.digest,
        reference_digest: frame.reference.digest,
        supervision_digest: frame.supervision.digest,
        asset: identity.asset.into(),
        session_start_time_ms: identity.start_time_ms,
        session_end_time_ms: identity.end_time_ms,
        condition_id: identity.condition_id.clone(),
        up_token: TokenKey::new(&identity.condition_id, &identity.up_token_id)
            .map_err(|_| CaptureError::Identifier)?,
        down_token: TokenKey::new(&identity.condition_id, &identity.down_token_id)
            .map_err(|_| CaptureError::Identifier)?,
        current: coordinator.current.get(&identity.asset) == Some(&key),
        active_ready: !coordinator.halted
            && session.ready
            && session.phase == SessionPhase::ActiveReady
            && frame.supervision.ready,
        up_book: source.up_book.map(ProposalBook::from),
        down_book: source.down_book.map(ProposalBook::from),
        context_digest: [0; 32],
    };
    context.context_digest = context_digest(&context);
    Ok(context)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalIntent {
    pub proposal_id: ProposalId,
    pub strategy: StrategyClass,
    pub token: TokenKey,
    pub side: OrderSide,
    pub quantity_micros: i128,
    pub partial_fill_micros: i128,
    pub limit_price_micros: i64,
    pub max_fee_micros: i128,
    pub evaluated_at_ns: i64,
    pub expires_at_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ProposalCommand {
    Evaluate {
        command_id: ProposalCommandId,
        context: Box<StrategyContext>,
        intent: Box<ProposalIntent>,
        recorded_at_ns: i64,
    },
}

impl ProposalCommand {
    #[must_use]
    pub const fn command_id(&self) -> ProposalCommandId {
        match self {
            Self::Evaluate { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Evaluate { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: ProposalCommand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Candidate,
    Reject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalReason {
    ContextAccepted,
    SourceNotReady,
    ContextExpired,
    IntentExpired,
    UnknownToken,
    InvalidEconomics,
    ProposalAlreadyUsed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalDecision {
    pub command_id: ProposalCommandId,
    pub proposal_id: ProposalId,
    pub status: ProposalStatus,
    pub reason: ProposalReason,
    pub context_digest: [u8; 32],
    pub intent_digest: [u8; 32],
    pub candidate: Option<OrderExposure>,
    pub decision_digest: [u8; 32],
}

impl ProposalDecision {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == decision_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalSnapshot {
    pub accepted_commands: u64,
    pub proposal_count: usize,
    pub candidate_count: u64,
    pub reject_count: u64,
    pub last_context_captured_at_ns: Option<i64>,
    pub last_context_digest: Option<[u8; 32]>,
    pub last_decision: Option<ProposalDecision>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("proposal command timestamp is invalid")]
    Timestamp,
    #[error("proposal command exceeds its canonical bound")]
    CommandBound,
    #[error("proposal command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported proposal command version: {0}")]
    Version(u16),
    #[error("proposal command id was reused for different content")]
    IdempotencyConflict,
    #[error("proposal command clock regressed")]
    ClockRegression,
    #[error("strategy context checksum is invalid")]
    ContextDigest,
    #[error("strategy context history regressed or equivocated")]
    ContextHistory,
    #[error("proposal arithmetic or counter overflow")]
    Overflow,
    #[error("proposal engine is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug, Default)]
pub struct ProposalEngine {
    processed: BTreeMap<ProposalCommandId, ([u8; 32], ProposalDecision)>,
    used_proposals: BTreeSet<ProposalId>,
    accepted_commands: u64,
    candidate_count: u64,
    reject_count: u64,
    last_recorded_at_ns: Option<i64>,
    last_context_captured_at_ns: Option<i64>,
    last_context_digest: Option<[u8; 32]>,
    last_decision: Option<ProposalDecision>,
    halted: Option<String>,
}

impl ProposalEngine {
    /// Applies one proposal command transactionally.
    ///
    /// # Errors
    ///
    /// Returns absorbing integrity, history, canonical, or arithmetic failures.
    pub fn apply(&mut self, command: &ProposalCommand) -> Result<ProposalDecision, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        let encoded = encode_command(command)?;
        let content = *blake3::hash(&encoded).as_bytes();
        let id = command.command_id();
        if let Some((existing, decision)) = self.processed.get(&id) {
            if *existing == content {
                return Ok(decision.clone());
            }
            return self.halt(Error::IdempotencyConflict);
        }
        if self
            .last_recorded_at_ns
            .is_some_and(|previous| command.recorded_at_ns() < previous)
        {
            return self.halt(Error::ClockRegression);
        }
        let ProposalCommand::Evaluate {
            context, intent, ..
        } = command;
        if !context.verify_digest() {
            return self.halt(Error::ContextDigest);
        }
        if self
            .last_context_captured_at_ns
            .is_some_and(|previous| context.captured_at_ns < previous)
            || (self.last_context_captured_at_ns == Some(context.captured_at_ns)
                && self
                    .last_context_digest
                    .is_some_and(|digest| digest != context.context_digest))
        {
            return self.halt(Error::ContextHistory);
        }
        let mut candidate = self.clone();
        let decision = match candidate.evaluate(id, context, intent) {
            Ok(decision) => decision,
            Err(Error::Overflow) => return self.halt(Error::Overflow),
            Err(error) => return Err(error),
        };
        candidate.accepted_commands = match candidate.accepted_commands.checked_add(1) {
            Some(count) => count,
            None => return self.halt(Error::Overflow),
        };
        candidate.last_recorded_at_ns = Some(command.recorded_at_ns());
        candidate.last_context_captured_at_ns = Some(context.captured_at_ns);
        candidate.last_context_digest = Some(context.context_digest);
        candidate.last_decision = Some(decision.clone());
        candidate.processed.insert(id, (content, decision.clone()));
        *self = candidate;
        Ok(decision)
    }

    #[must_use]
    pub fn snapshot(&self) -> ProposalSnapshot {
        ProposalSnapshot {
            accepted_commands: self.accepted_commands,
            proposal_count: self.used_proposals.len(),
            candidate_count: self.candidate_count,
            reject_count: self.reject_count,
            last_context_captured_at_ns: self.last_context_captured_at_ns,
            last_context_digest: self.last_context_digest,
            last_decision: self.last_decision.clone(),
            halted: self.halted.is_some(),
            halt_reason: self.halted.clone(),
            digest: self.state_digest(),
        }
    }

    #[must_use]
    pub const fn is_halted(&self) -> bool {
        self.halted.is_some()
    }

    fn evaluate(
        &mut self,
        command_id: ProposalCommandId,
        context: &StrategyContext,
        intent: &ProposalIntent,
    ) -> Result<ProposalDecision, Error> {
        let intent_digest = hash_serialized(b"strategy-proposal-intent-v1", intent);
        let reason = if !self.used_proposals.insert(intent.proposal_id) {
            ProposalReason::ProposalAlreadyUsed
        } else if !context.current
            || !context.active_ready
            || !books_ready(context.up_book, context.down_book)
        {
            ProposalReason::SourceNotReady
        } else if intent.evaluated_at_ns > context.valid_until_ns {
            ProposalReason::ContextExpired
        } else if intent.evaluated_at_ns > intent.expires_at_ns {
            ProposalReason::IntentExpired
        } else if intent.token != context.up_token && intent.token != context.down_token {
            ProposalReason::UnknownToken
        } else if !valid_economics(intent) {
            ProposalReason::InvalidEconomics
        } else {
            ProposalReason::ContextAccepted
        };
        let status = if reason == ProposalReason::ContextAccepted {
            ProposalStatus::Candidate
        } else {
            ProposalStatus::Reject
        };
        let candidate = (status == ProposalStatus::Candidate).then(|| OrderExposure {
            order_id: derived_order_id(intent.proposal_id, context.context_digest, intent_digest),
            token: intent.token.clone(),
            side: intent.side,
            quantity_micros: intent.quantity_micros,
            partial_fill_micros: intent.partial_fill_micros,
            limit_price_micros: intent.limit_price_micros,
            max_fee_micros: intent.max_fee_micros,
        });
        if status == ProposalStatus::Candidate {
            self.candidate_count = self.candidate_count.checked_add(1).ok_or(Error::Overflow)?;
        } else {
            self.reject_count = self.reject_count.checked_add(1).ok_or(Error::Overflow)?;
        }
        let mut decision = ProposalDecision {
            command_id,
            proposal_id: intent.proposal_id,
            status,
            reason,
            context_digest: context.context_digest,
            intent_digest,
            candidate,
            decision_digest: [0; 32],
        };
        decision.decision_digest = decision_digest(&decision);
        Ok(decision)
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"strategy-proposal-state-v1");
        for proposal in &self.used_proposals {
            hasher.update(&proposal.0);
        }
        for (id, (content, decision)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_into(&mut hasher, decision);
        }
        hash_into(&mut hasher, &self.accepted_commands);
        hash_into(&mut hasher, &self.candidate_count);
        hash_into(&mut hasher, &self.reject_count);
        hash_into(&mut hasher, &self.last_recorded_at_ns);
        hash_into(&mut hasher, &self.last_context_captured_at_ns);
        hash_into(&mut hasher, &self.last_context_digest);
        hash_into(&mut hasher, &self.last_decision);
        hash_into(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }

    fn halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }
}

fn books_ready(up: Option<ProposalBook>, down: Option<ProposalBook>) -> bool {
    [up, down].iter().all(|book| {
        book.is_some_and(|value| {
            value.authoritative && value.best_bid.is_some() && value.best_ask.is_some()
        })
    })
}

fn valid_economics(intent: &ProposalIntent) -> bool {
    intent.quantity_micros > 0
        && intent.quantity_micros <= MAX_QUANTITY_MICROS
        && intent.partial_fill_micros > 0
        && intent.partial_fill_micros <= intent.quantity_micros
        && (0..=1_000_000).contains(&intent.limit_price_micros)
        && (0..=MAX_FEE_MICROS).contains(&intent.max_fee_micros)
        && intent.evaluated_at_ns >= 0
        && intent.expires_at_ns >= 0
}

fn derived_order_id(
    proposal_id: ProposalId,
    context_digest: [u8; 32],
    intent_digest: [u8; 32],
) -> RiskOrderId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"strategy-proposal-risk-order-v1");
    hasher.update(&proposal_id.0);
    hasher.update(&context_digest);
    hasher.update(&intent_digest);
    RiskOrderId(*hasher.finalize().as_bytes())
}

fn context_digest(context: &StrategyContext) -> [u8; 32] {
    let mut value = context.clone();
    value.context_digest = [0; 32];
    hash_serialized(b"strategy-proposal-context-v1", &value)
}

fn decision_digest(decision: &ProposalDecision) -> [u8; 32] {
    let mut value = decision.clone();
    value.decision_digest = [0; 32];
    hash_serialized(b"strategy-proposal-decision-v1", &value)
}

fn hash_serialized<T: Serialize>(tag: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tag);
    hash_into(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_into<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("accepted proposal state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

fn validate_command(command: &ProposalCommand) -> Result<(), Error> {
    let ProposalCommand::Evaluate {
        context,
        intent,
        recorded_at_ns,
        ..
    } = command;
    if *recorded_at_ns < 0
        || intent.evaluated_at_ns != *recorded_at_ns
        || context.captured_at_ns > *recorded_at_ns
    {
        return Err(Error::Timestamp);
    }
    Ok(())
}

/// Encodes one bounded canonical proposal command.
///
/// # Errors
///
/// Rejects timestamp mismatch and oversized commands.
pub fn encode_command(command: &ProposalCommand) -> Result<Vec<u8>, Error> {
    validate_command(command)?;
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

/// Decodes one exact bounded proposal command.
///
/// # Errors
///
/// Rejects malformed, trailing, oversized, unsupported, or invalid input.
pub fn decode_command(bytes: &[u8]) -> Result<ProposalCommand, Error> {
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
    validate_command(&wire.command)?;
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
