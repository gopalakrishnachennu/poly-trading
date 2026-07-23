#![forbid(unsafe_code)]

//! Deterministic trade-settlement lifecycle and three-way reconciliation.
//!
//! The crate accepts typed observations. It contains no authenticated network,
//! RPC, wallet, signing, order, cancellation, or ledger-posting capability.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableReconciler,
    ReconciliationCheckpoint, ReconciliationRecovery, StorageError,
};

use accounting_ledger::{
    AccountingLedger, CommandId as LedgerCommandId, LedgerReconciliationView, TokenKey,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_TEXT_BYTES: usize = 512;
const MAX_COMMAND_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ReconciliationCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct IntentId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

/// Immutable local expectation for one matched trade, not an order-submission
/// request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradeIntent {
    pub intent_id: IntentId,
    pub trade_id: String,
    pub order_id: String,
    pub token: TokenKey,
    pub side: Side,
    pub quantity_micros: i128,
    pub consideration_micros: i128,
    pub fee_micros: i128,
    pub ledger_command_id: LedgerCommandId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradeStatus {
    Matched,
    Mined,
    Confirmed,
    Retrying,
    Failed,
}

impl TradeStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Confirmed | Self::Failed)
    }

    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Confirmed)
    }
}

/// One normalized CLOB trade event. Trade economics are repeated so every
/// update can be checked against immutable local intent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradeObservation {
    pub trade_id: String,
    pub order_id: String,
    pub token: TokenKey,
    pub side: Side,
    pub quantity_micros: i128,
    pub consideration_micros: i128,
    pub fee_micros: i128,
    pub status: TradeStatus,
    pub transaction_hash: Option<String>,
    pub matched_at_ns: i64,
    pub updated_at_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainTokenBalance {
    pub token: TokenKey,
    pub balance_micros: i128,
}

/// Finalized, externally supplied blockchain view. Non-final blocks have no
/// representation in the reconciliation command language.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalizedChainSnapshot {
    pub chain_id: u64,
    pub wallet: String,
    pub block_number: u64,
    pub block_hash: String,
    pub finalized_at_ns: i64,
    pub observed_at_ns: i64,
    pub collateral_micros: i128,
    pub token_balances: Vec<ChainTokenBalance>,
}

/// Atomic comparison of Phase 2.0 ledger state and finalized chain state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationFrame {
    pub ledger: LedgerReconciliationView,
    pub chain: FinalizedChainSnapshot,
    pub evaluated_at_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ReconciliationCommand {
    RegisterIntent {
        command_id: ReconciliationCommandId,
        intent: TradeIntent,
        recorded_at_ns: i64,
    },
    ObserveTrade {
        command_id: ReconciliationCommandId,
        observation: TradeObservation,
        recorded_at_ns: i64,
    },
    Reconcile {
        command_id: ReconciliationCommandId,
        frame: ReconciliationFrame,
        recorded_at_ns: i64,
    },
}

impl ReconciliationCommand {
    #[must_use]
    pub const fn command_id(&self) -> ReconciliationCommandId {
        match self {
            Self::RegisterIntent { command_id, .. }
            | Self::ObserveTrade { command_id, .. }
            | Self::Reconcile { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::RegisterIntent { recorded_at_ns, .. }
            | Self::ObserveTrade { recorded_at_ns, .. }
            | Self::Reconcile { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: ReconciliationCommand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconcilerConfig {
    pub chain_id: u64,
    pub wallet: String,
    pub confirmation_grace_ns: i64,
    pub max_intents: usize,
    pub max_tokens: usize,
}

impl ReconcilerConfig {
    /// Validates immutable reconciliation identity and hard bounds.
    ///
    /// # Errors
    ///
    /// Rejects empty/oversized identity, zero chain, negative grace, or zero
    /// collection bounds.
    pub fn validate(&self) -> Result<(), Error> {
        validate_text(&self.wallet)?;
        if self.chain_id == 0
            || self.confirmation_grace_ns < 0
            || self.max_intents == 0
            || self.max_tokens == 0
        {
            return Err(Error::Config);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationMode {
    AwaitingSources,
    Pending,
    Reconciled,
    Halted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    Applied,
    Duplicate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationSnapshot {
    pub mode: ReconciliationMode,
    pub ready: bool,
    pub accepted_commands: u64,
    pub intent_count: usize,
    pub observed_trade_count: usize,
    pub nonterminal_trade_count: usize,
    pub confirmed_trade_count: usize,
    pub failed_trade_count: usize,
    pub last_evaluated_at_ns: Option<i64>,
    pub ledger_digest: Option<[u8; 32]>,
    pub chain_block_number: Option<u64>,
    pub chain_block_hash: Option<String>,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

/// Minimal immutable proof consumed by the offline portfolio risk gate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationRiskGate {
    pub reconciliation_digest: [u8; 32],
    pub ready: bool,
    pub evaluated_at_ns: Option<i64>,
    pub ledger_digest: Option<[u8; 32]>,
    pub chain_block_number: Option<u64>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("reconciler configuration is invalid")]
    Config,
    #[error("identifier is empty or exceeds its bound")]
    Identifier,
    #[error("amount must be strictly positive")]
    NonPositive,
    #[error("fee is negative or exceeds consideration")]
    Fee,
    #[error("timestamp is negative or internally inconsistent")]
    Timestamp,
    #[error("collection exceeds its configured bound")]
    Bound,
    #[error("token balances are not strictly sorted, unique, positive, and bounded")]
    TokenBalances,
    #[error("intent identity, trade ID, or expected ledger command is reused")]
    IntentConflict,
    #[error("CLOB observation does not map to a registered local intent")]
    UnknownTrade,
    #[error("CLOB trade economics changed after registration")]
    TradeFactsChanged,
    #[error("CLOB trade status transition is impossible")]
    StatusTransition,
    #[error("terminal CLOB trade state changed")]
    TerminalMutation,
    #[error("mined or confirmed trade lacks an on-chain transaction hash")]
    TransactionHash,
    #[error("trade update time regressed")]
    TradeTimeRegression,
    #[error("ledger source is halted")]
    LedgerHalted,
    #[error("ledger history regressed or equivocated")]
    LedgerHistory,
    #[error("blockchain identity does not match configured chain and wallet")]
    ChainIdentity,
    #[error("finalized blockchain history regressed or equivocated")]
    ChainHistory,
    #[error("ledger and finalized blockchain collateral differ")]
    CollateralMismatch,
    #[error("ledger and finalized blockchain token balances differ")]
    TokenMismatch,
    #[error("unconfirmed or failed trade has already been posted to the ledger")]
    PrematurePosting,
    #[error("confirmed trade exceeded its ledger-posting grace interval")]
    ConfirmationExpired,
    #[error("ledger view contains an unrelated command ID")]
    LedgerCommandSet,
    #[error("arithmetic overflow")]
    Overflow,
    #[error("command idempotency key was reused for different content")]
    IdempotencyConflict,
    #[error("reconciler is halted: {0}")]
    Halted(String),
    #[error("command exceeds the canonical payload bound")]
    CommandBound,
    #[error("command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported command version: {0}")]
    Version(u16),
}

/// Deterministic single-writer reconciliation state.
#[derive(Clone, Debug)]
pub struct SettlementReconciler {
    config: ReconcilerConfig,
    intents: BTreeMap<IntentId, TradeIntent>,
    trade_to_intent: BTreeMap<String, IntentId>,
    ledger_command_to_intent: BTreeMap<LedgerCommandId, IntentId>,
    trades: BTreeMap<String, TradeObservation>,
    last_frame: Option<ReconciliationFrame>,
    mode: ReconciliationMode,
    processed: BTreeMap<ReconciliationCommandId, [u8; 32]>,
    accepted_commands: u64,
    halted: Option<String>,
}

impl SettlementReconciler {
    /// Creates an empty reconciler with immutable identity and limits.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration.
    pub fn new(config: ReconcilerConfig) -> Result<Self, Error> {
        config.validate()?;
        Ok(Self {
            config,
            intents: BTreeMap::new(),
            trade_to_intent: BTreeMap::new(),
            ledger_command_to_intent: BTreeMap::new(),
            trades: BTreeMap::new(),
            last_frame: None,
            mode: ReconciliationMode::AwaitingSources,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            halted: None,
        })
    }

    /// Applies one canonical command. Integrity failures install an absorbing
    /// halt while preserving every previously accepted fact.
    ///
    /// # Errors
    ///
    /// Returns validation or integrity errors. Content-identical command IDs
    /// are no-ops.
    pub fn apply(&mut self, command: &ReconciliationCommand) -> Result<ApplyOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        let bytes = encode_command(command)?;
        let digest = *blake3::hash(&bytes).as_bytes();
        let command_id = command.command_id();
        if let Some(existing) = self.processed.get(&command_id) {
            if *existing == digest {
                return Ok(ApplyOutcome::Duplicate);
            }
            return self.install_halt(Error::IdempotencyConflict);
        }

        let mut candidate = self.clone();
        if let Err(error) = candidate.apply_fresh(command) {
            candidate.halted = Some(error.to_string());
            candidate.mode = ReconciliationMode::Halted;
            candidate.processed.insert(command_id, digest);
            candidate.accepted_commands = candidate
                .accepted_commands
                .checked_add(1)
                .ok_or(Error::Overflow)?;
            *self = candidate;
            return Err(error);
        }
        candidate.processed.insert(command_id, digest);
        candidate.accepted_commands = candidate
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        *self = candidate;
        Ok(ApplyOutcome::Applied)
    }

    /// Constructs an atomic frame from the actual Phase 2.0 ledger and a
    /// caller-supplied finalized blockchain snapshot.
    #[must_use]
    pub fn capture_frame(
        &self,
        ledger: &AccountingLedger,
        chain: FinalizedChainSnapshot,
        evaluated_at_ns: i64,
    ) -> ReconciliationFrame {
        let ids = self
            .intents
            .values()
            .map(|intent| intent.ledger_command_id)
            .collect();
        ReconciliationFrame {
            ledger: ledger.reconciliation_view(&ids),
            chain,
            evaluated_at_ns,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ReconciliationSnapshot {
        let nonterminal = self
            .trades
            .values()
            .filter(|trade| !trade.status.is_terminal())
            .count();
        let confirmed = self
            .trades
            .values()
            .filter(|trade| trade.status == TradeStatus::Confirmed)
            .count();
        let failed = self
            .trades
            .values()
            .filter(|trade| trade.status == TradeStatus::Failed)
            .count();
        ReconciliationSnapshot {
            mode: self.mode,
            ready: self.mode == ReconciliationMode::Reconciled,
            accepted_commands: self.accepted_commands,
            intent_count: self.intents.len(),
            observed_trade_count: self.trades.len(),
            nonterminal_trade_count: nonterminal,
            confirmed_trade_count: confirmed,
            failed_trade_count: failed,
            last_evaluated_at_ns: self.last_frame.as_ref().map(|frame| frame.evaluated_at_ns),
            ledger_digest: self
                .last_frame
                .as_ref()
                .map(|frame| frame.ledger.ledger_digest),
            chain_block_number: self
                .last_frame
                .as_ref()
                .map(|frame| frame.chain.block_number),
            chain_block_hash: self
                .last_frame
                .as_ref()
                .map(|frame| frame.chain.block_hash.clone()),
            halt_reason: self.halted.clone(),
            digest: self.state_digest(),
        }
    }

    #[must_use]
    pub const fn is_halted(&self) -> bool {
        self.halted.is_some()
    }

    /// Returns the latest authoritative observation for a registered trade.
    #[must_use]
    pub fn trade(&self, trade_id: &str) -> Option<&TradeObservation> {
        self.trades.get(trade_id)
    }

    /// Captures the exact reconciliation provenance required by pre-trade risk.
    #[must_use]
    pub fn risk_gate(&self) -> ReconciliationRiskGate {
        let snapshot = self.snapshot();
        ReconciliationRiskGate {
            reconciliation_digest: snapshot.digest,
            ready: snapshot.ready,
            evaluated_at_ns: snapshot.last_evaluated_at_ns,
            ledger_digest: snapshot.ledger_digest,
            chain_block_number: snapshot.chain_block_number,
        }
    }

    fn apply_fresh(&mut self, command: &ReconciliationCommand) -> Result<(), Error> {
        match command {
            ReconciliationCommand::RegisterIntent { intent, .. } => self.register(intent.clone()),
            ReconciliationCommand::ObserveTrade { observation, .. } => {
                self.observe_trade(observation.clone())
            }
            ReconciliationCommand::Reconcile { frame, .. } => self.reconcile(frame.clone()),
        }
    }

    fn register(&mut self, intent: TradeIntent) -> Result<(), Error> {
        validate_intent(&intent)?;
        if self.intents.len() >= self.config.max_intents {
            return Err(Error::Bound);
        }
        if self.intents.contains_key(&intent.intent_id)
            || self.trade_to_intent.contains_key(&intent.trade_id)
            || self
                .ledger_command_to_intent
                .contains_key(&intent.ledger_command_id)
        {
            return Err(Error::IntentConflict);
        }
        self.trade_to_intent
            .insert(intent.trade_id.clone(), intent.intent_id);
        self.ledger_command_to_intent
            .insert(intent.ledger_command_id, intent.intent_id);
        self.intents.insert(intent.intent_id, intent);
        self.mode = ReconciliationMode::Pending;
        Ok(())
    }

    fn observe_trade(&mut self, observation: TradeObservation) -> Result<(), Error> {
        validate_observation(&observation)?;
        let intent_id = self
            .trade_to_intent
            .get(&observation.trade_id)
            .copied()
            .ok_or(Error::UnknownTrade)?;
        let intent = self.intents.get(&intent_id).ok_or(Error::UnknownTrade)?;
        if !facts_match(intent, &observation) {
            return Err(Error::TradeFactsChanged);
        }
        if let Some(previous) = self.trades.get(&observation.trade_id) {
            validate_transition(previous, &observation)?;
        } else if observation.status != TradeStatus::Matched {
            return Err(Error::StatusTransition);
        }
        self.trades
            .insert(observation.trade_id.clone(), observation);
        self.mode = ReconciliationMode::Pending;
        Ok(())
    }

    fn reconcile(&mut self, frame: ReconciliationFrame) -> Result<(), Error> {
        validate_frame(&frame, &self.config)?;
        self.validate_history(&frame)?;
        if frame.ledger.halted {
            return Err(Error::LedgerHalted);
        }
        let expected_ids: BTreeSet<_> = self
            .intents
            .values()
            .map(|intent| intent.ledger_command_id)
            .collect();
        if !frame.ledger.present_command_ids.is_subset(&expected_ids) {
            return Err(Error::LedgerCommandSet);
        }
        if frame.ledger.collateral_micros != frame.chain.collateral_micros {
            return Err(Error::CollateralMismatch);
        }
        if ledger_tokens(&frame.ledger)? != chain_tokens(&frame.chain)? {
            return Err(Error::TokenMismatch);
        }

        let mut pending = false;
        for intent in self.intents.values() {
            let posted = frame
                .ledger
                .present_command_ids
                .contains(&intent.ledger_command_id);
            let Some(trade) = self.trades.get(&intent.trade_id) else {
                if posted {
                    return Err(Error::PrematurePosting);
                }
                pending = true;
                continue;
            };
            match trade.status {
                TradeStatus::Matched | TradeStatus::Mined | TradeStatus::Retrying => {
                    if posted {
                        return Err(Error::PrematurePosting);
                    }
                    pending = true;
                }
                TradeStatus::Failed => {
                    if posted {
                        return Err(Error::PrematurePosting);
                    }
                }
                TradeStatus::Confirmed => {
                    if !posted {
                        let age = frame
                            .evaluated_at_ns
                            .checked_sub(trade.updated_at_ns)
                            .ok_or(Error::Overflow)?;
                        if age > self.config.confirmation_grace_ns {
                            return Err(Error::ConfirmationExpired);
                        }
                        pending = true;
                    }
                }
            }
        }
        self.mode = if pending {
            ReconciliationMode::Pending
        } else {
            ReconciliationMode::Reconciled
        };
        self.last_frame = Some(frame);
        Ok(())
    }

    fn validate_history(&self, frame: &ReconciliationFrame) -> Result<(), Error> {
        let Some(previous) = &self.last_frame else {
            return Ok(());
        };
        if frame.evaluated_at_ns < previous.evaluated_at_ns {
            return Err(Error::Timestamp);
        }
        if frame.ledger.accepted_commands < previous.ledger.accepted_commands
            || (frame.ledger.accepted_commands == previous.ledger.accepted_commands
                && frame.ledger.ledger_digest != previous.ledger.ledger_digest)
        {
            return Err(Error::LedgerHistory);
        }
        if frame.chain.block_number < previous.chain.block_number {
            return Err(Error::ChainHistory);
        }
        if frame.chain.block_number == previous.chain.block_number
            && (frame.chain.block_hash != previous.chain.block_hash
                || frame.chain.collateral_micros != previous.chain.collateral_micros
                || frame.chain.token_balances != previous.chain.token_balances)
        {
            return Err(Error::ChainHistory);
        }
        if frame.chain.observed_at_ns < previous.chain.observed_at_ns
            || frame.chain.finalized_at_ns < previous.chain.finalized_at_ns
        {
            return Err(Error::ChainHistory);
        }
        Ok(())
    }

    fn install_halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        self.mode = ReconciliationMode::Halted;
        Err(error)
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"settlement-reconciliation-state-v1");
        hasher.update(&self.config.chain_id.to_le_bytes());
        hash_json(&mut hasher, &self.config.wallet);
        hasher.update(&self.config.confirmation_grace_ns.to_le_bytes());
        hasher.update(&self.accepted_commands.to_le_bytes());
        for (id, intent) in &self.intents {
            hasher.update(&id.0);
            hash_json(&mut hasher, intent);
        }
        for (trade_id, observation) in &self.trades {
            hash_json(&mut hasher, trade_id);
            hash_json(&mut hasher, observation);
        }
        hash_json(&mut hasher, &self.last_frame);
        for (id, digest) in &self.processed {
            hasher.update(&id.0);
            hasher.update(digest);
        }
        hasher.update(&[mode_tag(self.mode)]);
        hash_json(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }
}

fn validate_intent(intent: &TradeIntent) -> Result<(), Error> {
    validate_text(&intent.trade_id)?;
    validate_text(&intent.order_id)?;
    validate_token(&intent.token)?;
    positive(intent.quantity_micros)?;
    positive(intent.consideration_micros)?;
    validate_fee(intent.fee_micros, intent.consideration_micros)
}

fn validate_observation(observation: &TradeObservation) -> Result<(), Error> {
    validate_text(&observation.trade_id)?;
    validate_text(&observation.order_id)?;
    validate_token(&observation.token)?;
    positive(observation.quantity_micros)?;
    positive(observation.consideration_micros)?;
    validate_fee(observation.fee_micros, observation.consideration_micros)?;
    if observation.matched_at_ns < 0 || observation.updated_at_ns < observation.matched_at_ns {
        return Err(Error::Timestamp);
    }
    if let Some(hash) = &observation.transaction_hash {
        validate_text(hash)?;
    }
    if matches!(
        observation.status,
        TradeStatus::Mined | TradeStatus::Confirmed
    ) && observation.transaction_hash.is_none()
    {
        return Err(Error::TransactionHash);
    }
    Ok(())
}

fn validate_frame(frame: &ReconciliationFrame, config: &ReconcilerConfig) -> Result<(), Error> {
    if frame.evaluated_at_ns < 0
        || frame.chain.finalized_at_ns < 0
        || frame.chain.observed_at_ns < frame.chain.finalized_at_ns
        || frame.evaluated_at_ns < frame.chain.observed_at_ns
    {
        return Err(Error::Timestamp);
    }
    if frame.chain.chain_id != config.chain_id || frame.chain.wallet != config.wallet {
        return Err(Error::ChainIdentity);
    }
    validate_text(&frame.chain.wallet)?;
    validate_text(&frame.chain.block_hash)?;
    if frame.chain.collateral_micros < 0
        || frame.ledger.collateral_micros < 0
        || frame.chain.token_balances.len() > config.max_tokens
        || frame.ledger.token_balances.len() > config.max_tokens
        || frame.ledger.present_command_ids.len() > config.max_intents
    {
        return Err(Error::Bound);
    }
    ledger_tokens(&frame.ledger)?;
    chain_tokens(&frame.chain)?;
    Ok(())
}

fn ledger_tokens(view: &LedgerReconciliationView) -> Result<BTreeMap<TokenKey, i128>, Error> {
    let mut result = BTreeMap::new();
    for balance in &view.token_balances {
        validate_token(&balance.token)?;
        if balance.balance_micros <= 0
            || result
                .insert(balance.token.clone(), balance.balance_micros)
                .is_some()
        {
            return Err(Error::TokenBalances);
        }
    }
    if result
        .keys()
        .ne(view.token_balances.iter().map(|balance| &balance.token))
    {
        return Err(Error::TokenBalances);
    }
    Ok(result)
}

fn chain_tokens(snapshot: &FinalizedChainSnapshot) -> Result<BTreeMap<TokenKey, i128>, Error> {
    let mut result = BTreeMap::new();
    for balance in &snapshot.token_balances {
        validate_token(&balance.token)?;
        if balance.balance_micros <= 0
            || result
                .insert(balance.token.clone(), balance.balance_micros)
                .is_some()
        {
            return Err(Error::TokenBalances);
        }
    }
    if result
        .keys()
        .ne(snapshot.token_balances.iter().map(|balance| &balance.token))
    {
        return Err(Error::TokenBalances);
    }
    Ok(result)
}

fn facts_match(intent: &TradeIntent, observation: &TradeObservation) -> bool {
    intent.trade_id == observation.trade_id
        && intent.order_id == observation.order_id
        && intent.token == observation.token
        && intent.side == observation.side
        && intent.quantity_micros == observation.quantity_micros
        && intent.consideration_micros == observation.consideration_micros
        && intent.fee_micros == observation.fee_micros
}

fn validate_transition(previous: &TradeObservation, next: &TradeObservation) -> Result<(), Error> {
    if previous.status.is_terminal() {
        return if previous == next {
            Ok(())
        } else {
            Err(Error::TerminalMutation)
        };
    }
    if next.updated_at_ns < previous.updated_at_ns {
        return Err(Error::TradeTimeRegression);
    }
    let allowed = previous.status == next.status
        || matches!(
            (previous.status, next.status),
            (
                TradeStatus::Matched,
                TradeStatus::Mined | TradeStatus::Retrying
            ) | (
                TradeStatus::Mined,
                TradeStatus::Confirmed | TradeStatus::Retrying
            ) | (
                TradeStatus::Retrying,
                TradeStatus::Mined | TradeStatus::Failed
            )
        );
    if !allowed {
        return Err(Error::StatusTransition);
    }
    if previous.matched_at_ns != next.matched_at_ns {
        return Err(Error::TradeFactsChanged);
    }
    if previous.status == TradeStatus::Mined
        && next.status == TradeStatus::Confirmed
        && previous.transaction_hash != next.transaction_hash
    {
        return Err(Error::TradeFactsChanged);
    }
    if previous.status == next.status && previous.transaction_hash != next.transaction_hash {
        return Err(Error::TradeFactsChanged);
    }
    Ok(())
}

fn validate_command(command: &ReconciliationCommand) -> Result<(), Error> {
    if command.recorded_at_ns() < 0 {
        return Err(Error::Timestamp);
    }
    match command {
        ReconciliationCommand::RegisterIntent { intent, .. } => validate_intent(intent),
        ReconciliationCommand::ObserveTrade {
            observation,
            recorded_at_ns,
            ..
        } => {
            validate_observation(observation)?;
            if *recorded_at_ns < observation.updated_at_ns {
                return Err(Error::Timestamp);
            }
            Ok(())
        }
        ReconciliationCommand::Reconcile {
            frame,
            recorded_at_ns,
            ..
        } => {
            if frame.evaluated_at_ns < 0 || *recorded_at_ns < frame.evaluated_at_ns {
                Err(Error::Timestamp)
            } else {
                Ok(())
            }
        }
    }
}

fn validate_token(token: &TokenKey) -> Result<(), Error> {
    validate_text(&token.condition_id)?;
    validate_text(&token.token_id)
}

fn validate_text(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES {
        Err(Error::Identifier)
    } else {
        Ok(())
    }
}

fn positive(value: i128) -> Result<(), Error> {
    if value > 0 {
        Ok(())
    } else {
        Err(Error::NonPositive)
    }
}

fn validate_fee(fee: i128, consideration: i128) -> Result<(), Error> {
    if fee < 0 || fee > consideration {
        Err(Error::Fee)
    } else {
        Ok(())
    }
}

fn mode_tag(mode: ReconciliationMode) -> u8 {
    match mode {
        ReconciliationMode::AwaitingSources => 0,
        ReconciliationMode::Pending => 1,
        ReconciliationMode::Reconciled => 2,
        ReconciliationMode::Halted => 3,
    }
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("internal reconciliation state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

/// Encodes one bounded versioned command.
///
/// # Errors
///
/// Rejects invalid command data, serialization failure, or an oversized payload.
pub fn encode_command(command: &ReconciliationCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one exact bounded versioned command.
///
/// # Errors
///
/// Rejects malformed JSON, unknown fields, unsupported versions, trailing data,
/// and invalid command values.
pub fn decode_command(bytes: &[u8]) -> Result<ReconciliationCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let wire: WireCommand =
        serde_json::from_slice(bytes).map_err(|error| Error::Json(error.to_string()))?;
    if wire.version != WIRE_VERSION {
        return Err(Error::Version(wire.version));
    }
    validate_command(&wire.command)?;
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
