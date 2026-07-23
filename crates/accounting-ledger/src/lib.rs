#![forbid(unsafe_code)]

//! Deterministic, per-asset double-entry accounting and capital reservations.
//!
//! This crate is deliberately offline. It cannot submit orders, sign messages,
//! inspect wallets, or turn pending exchange activity into confirmed assets.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableLedger,
    LedgerCheckpoint, LedgerRecovery, PersistenceError,
};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_CONFIRMATION_BYTES: usize = 512;
const MAX_COMMAND_BYTES: usize = 64 * 1024;

/// A content-bound idempotency key supplied by the command producer.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CommandId(pub [u8; 32]);

/// Immutable reservation identity.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ReservationId(pub [u8; 32]);

/// Immutable complete-pair lock identity.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct LockId(pub [u8; 32]);

/// Exact condition and outcome-token identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenKey {
    pub condition_id: String,
    pub token_id: String,
}

impl TokenKey {
    /// Creates a bounded, non-empty token identity.
    ///
    /// # Errors
    ///
    /// Rejects empty or oversized identifiers.
    pub fn new(
        condition_id: impl Into<String>,
        token_id: impl Into<String>,
    ) -> Result<Self, Error> {
        let value = Self {
            condition_id: condition_id.into(),
            token_id: token_id.into(),
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), Error> {
        for value in [&self.condition_id, &self.token_id] {
            if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
                return Err(Error::Identifier);
            }
        }
        Ok(())
    }
}

/// Unit carried by a posting. Different assets never offset each other.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Asset {
    Collateral,
    Outcome(TokenKey),
}

/// Ledger account. Positive values are debit balances; revenue and capital
/// normally carry negative credit balances.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Account {
    CashAvailable,
    CashReserved(ReservationId),
    TokenAvailable,
    TokenReserved(ReservationId),
    InventoryCost(TokenKey),
    LockedToken(LockId),
    LockedCost(LockId),
    FeeExpense,
    TradingRevenue,
    CostOfGoodsSold,
    CapitalContributed,
    External,
}

/// One signed fixed-point movement in micros.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Posting {
    pub account: Account,
    pub asset: Asset,
    pub delta_micros: i128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    postings: Vec<Posting>,
}

/// Reservation unit and owner.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ReservationAsset {
    Collateral,
    Token(TokenKey),
}

/// Reservation lifecycle. A partial consumption remains active.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationStatus {
    Active,
    Released,
    Consumed,
}

/// Immutable reservation identity plus its remaining controlled balance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reservation {
    pub id: ReservationId,
    pub asset: ReservationAsset,
    pub original_micros: i128,
    pub remaining_micros: i128,
    pub status: ReservationStatus,
}

/// Quantity and collateral cost still attached to an outcome token.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostPosition {
    pub quantity_micros: i128,
    pub cost_micros: i128,
}

/// Locked complete-pair lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockStatus {
    Active,
    Merged,
}

/// Equal confirmed Up/Down inventory whose payout is deterministic but not yet
/// spendable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairLock {
    pub id: LockId,
    pub up: TokenKey,
    pub down: TokenKey,
    pub quantity_micros: i128,
    pub cost_micros: i128,
    pub payout_micros: i128,
    pub status: LockStatus,
}

/// Canonical accounting commands. Confirmation references must identify an
/// authoritative external confirmation; pending states intentionally have no
/// command variant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LedgerCommand {
    FundCollateral {
        command_id: CommandId,
        amount_micros: i128,
        recorded_at_ns: i64,
    },
    ReserveCollateral {
        command_id: CommandId,
        reservation_id: ReservationId,
        amount_micros: i128,
        recorded_at_ns: i64,
    },
    ReserveToken {
        command_id: CommandId,
        reservation_id: ReservationId,
        token: TokenKey,
        quantity_micros: i128,
        recorded_at_ns: i64,
    },
    ReleaseReservation {
        command_id: CommandId,
        reservation_id: ReservationId,
        recorded_at_ns: i64,
    },
    ConfirmBuy {
        command_id: CommandId,
        reservation_id: ReservationId,
        token: TokenKey,
        quantity_micros: i128,
        consideration_micros: i128,
        fee_micros: i128,
        confirmation: String,
        recorded_at_ns: i64,
    },
    ConfirmSell {
        command_id: CommandId,
        reservation_id: ReservationId,
        quantity_micros: i128,
        gross_proceeds_micros: i128,
        fee_micros: i128,
        confirmation: String,
        recorded_at_ns: i64,
    },
    LockPair {
        command_id: CommandId,
        lock_id: LockId,
        up: TokenKey,
        down: TokenKey,
        quantity_micros: i128,
        recorded_at_ns: i64,
    },
    ConfirmMerge {
        command_id: CommandId,
        lock_id: LockId,
        confirmation: String,
        recorded_at_ns: i64,
    },
    ConfirmSplit {
        command_id: CommandId,
        reservation_id: ReservationId,
        up: TokenKey,
        down: TokenKey,
        quantity_micros: i128,
        confirmation: String,
        recorded_at_ns: i64,
    },
    ConfirmRedemption {
        command_id: CommandId,
        reservation_id: ReservationId,
        quantity_micros: i128,
        payout_micros: i128,
        confirmation: String,
        recorded_at_ns: i64,
    },
}

impl LedgerCommand {
    #[must_use]
    pub const fn command_id(&self) -> CommandId {
        match self {
            Self::FundCollateral { command_id, .. }
            | Self::ReserveCollateral { command_id, .. }
            | Self::ReserveToken { command_id, .. }
            | Self::ReleaseReservation { command_id, .. }
            | Self::ConfirmBuy { command_id, .. }
            | Self::ConfirmSell { command_id, .. }
            | Self::LockPair { command_id, .. }
            | Self::ConfirmMerge { command_id, .. }
            | Self::ConfirmSplit { command_id, .. }
            | Self::ConfirmRedemption { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::FundCollateral { recorded_at_ns, .. }
            | Self::ReserveCollateral { recorded_at_ns, .. }
            | Self::ReserveToken { recorded_at_ns, .. }
            | Self::ReleaseReservation { recorded_at_ns, .. }
            | Self::ConfirmBuy { recorded_at_ns, .. }
            | Self::ConfirmSell { recorded_at_ns, .. }
            | Self::LockPair { recorded_at_ns, .. }
            | Self::ConfirmMerge { recorded_at_ns, .. }
            | Self::ConfirmSplit { recorded_at_ns, .. }
            | Self::ConfirmRedemption { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: LedgerCommand,
}

/// Result of idempotent command application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    Applied,
    Duplicate,
}

/// Immutable accounting summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerSnapshot {
    pub accepted_commands: u64,
    pub cash_available_micros: i128,
    pub cash_reserved_micros: i128,
    pub fees_micros: i128,
    pub realized_net_pnl_micros: i128,
    pub locked_pnl_micros: i128,
    pub active_reservations: usize,
    pub active_locks: usize,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

/// Bounded-on-request view used by the read-only reconciliation kernel.
/// Token balances include available, reserved, and locked confirmed inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerReconciliationView {
    pub ledger_digest: [u8; 32],
    pub accepted_commands: u64,
    pub halted: bool,
    pub collateral_micros: i128,
    pub token_balances: Vec<ConfirmedTokenBalance>,
    pub present_command_ids: BTreeSet<CommandId>,
}

/// One exact confirmed outcome-token balance in a reconciliation view.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedTokenBalance {
    pub token: TokenKey,
    pub balance_micros: i128,
}

/// Confirmed asset categories required by conservative pre-trade risk.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRiskView {
    pub ledger_digest: [u8; 32],
    pub halted: bool,
    pub cash_available_micros: i128,
    pub cash_reserved_micros: i128,
    pub available_tokens: Vec<ConfirmedTokenBalance>,
    pub reserved_tokens: Vec<ConfirmedTokenBalance>,
    pub locked_tokens: Vec<ConfirmedTokenBalance>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("identifier is empty or exceeds its bound")]
    Identifier,
    #[error("amount must be strictly positive")]
    NonPositive,
    #[error("fee is negative or exceeds gross proceeds")]
    Fee,
    #[error("arithmetic overflow")]
    Overflow,
    #[error("transaction has fewer than two non-zero postings")]
    PostingCount,
    #[error("transaction is not balanced for asset {0:?}")]
    Unbalanced(Asset),
    #[error("account and asset are incompatible")]
    AccountAsset,
    #[error("controlled account would become negative")]
    NegativeBalance,
    #[error("reservation already exists")]
    ReservationExists,
    #[error("reservation does not exist")]
    ReservationMissing,
    #[error("reservation is not active")]
    ReservationInactive,
    #[error("reservation asset or owner does not match the command")]
    ReservationMismatch,
    #[error("reservation has insufficient remaining balance")]
    ReservationInsufficient,
    #[error("inventory is insufficient")]
    InventoryInsufficient,
    #[error("complete pair tokens must be distinct and share one condition")]
    PairIdentity,
    #[error("pair lock already exists")]
    LockExists,
    #[error("pair lock does not exist")]
    LockMissing,
    #[error("pair lock is not active")]
    LockInactive,
    #[error("confirmation reference is empty or exceeds its bound")]
    Confirmation,
    #[error("command timestamp is negative")]
    Timestamp,
    #[error("command exceeds the canonical payload bound")]
    CommandBound,
    #[error("command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported command version: {0}")]
    Version(u16),
    #[error("idempotency key was reused for different command content")]
    IdempotencyConflict,
    #[error("ledger is halted: {0}")]
    Halted(String),
    #[error("ledger conservation invariant failed")]
    Conservation,
    #[error("ledger reservation invariant failed")]
    ReservationInvariant,
    #[error("ledger inventory invariant failed")]
    InventoryInvariant,
}

/// Single-writer deterministic accounting state.
#[derive(Clone, Debug, Default)]
pub struct AccountingLedger {
    balances: BTreeMap<(Account, Asset), i128>,
    reservations: BTreeMap<ReservationId, Reservation>,
    costs: BTreeMap<TokenKey, CostPosition>,
    locks: BTreeMap<LockId, PairLock>,
    processed: BTreeMap<CommandId, [u8; 32]>,
    accepted_commands: u64,
    halted: Option<String>,
}

impl AccountingLedger {
    /// Applies one command transactionally.
    ///
    /// Exact command duplicates are no-ops. Reusing an idempotency key with
    /// different content permanently halts this instance.
    ///
    /// # Errors
    ///
    /// Rejects invalid commands, insufficient balances, overflow, or invariant
    /// failure without partial mutation.
    pub fn apply(&mut self, command: &LedgerCommand) -> Result<ApplyOutcome, Error> {
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
            let reason = Error::IdempotencyConflict.to_string();
            self.halted = Some(reason);
            return Err(Error::IdempotencyConflict);
        }

        let mut candidate = self.clone();
        candidate.apply_fresh(command)?;
        candidate.processed.insert(command_id, digest);
        candidate.accepted_commands = candidate
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        candidate.verify_invariants()?;
        *self = candidate;
        Ok(ApplyOutcome::Applied)
    }

    #[must_use]
    pub fn balance(&self, account: &Account, asset: &Asset) -> i128 {
        self.balances
            .get(&(account.clone(), asset.clone()))
            .copied()
            .unwrap_or(0)
    }

    #[must_use]
    pub fn reservation(&self, id: ReservationId) -> Option<&Reservation> {
        self.reservations.get(&id)
    }

    #[must_use]
    pub fn cost_position(&self, token: &TokenKey) -> CostPosition {
        self.costs.get(token).copied().unwrap_or(CostPosition {
            quantity_micros: 0,
            cost_micros: 0,
        })
    }

    #[must_use]
    pub fn pair_lock(&self, id: LockId) -> Option<&PairLock> {
        self.locks.get(&id)
    }

    /// Returns the immutable, stable accounting summary.
    ///
    /// # Panics
    ///
    /// Panics only if the private ledger state violates the checked P&L
    /// arithmetic invariant. All public mutation paths verify that invariant
    /// transactionally before installing state.
    #[must_use]
    pub fn snapshot(&self) -> LedgerSnapshot {
        let cash_available = self.balance(&Account::CashAvailable, &Asset::Collateral);
        let cash_reserved = self
            .reservations
            .values()
            .filter(|reservation| {
                reservation.status == ReservationStatus::Active
                    && reservation.asset == ReservationAsset::Collateral
            })
            .map(|reservation| reservation.remaining_micros)
            .sum();
        let (fees, realized_net_pnl, locked_pnl) = self
            .checked_pnl()
            .expect("accepted ledger state has checked P&L arithmetic");
        LedgerSnapshot {
            accepted_commands: self.accepted_commands,
            cash_available_micros: cash_available,
            cash_reserved_micros: cash_reserved,
            fees_micros: fees,
            realized_net_pnl_micros: realized_net_pnl,
            locked_pnl_micros: locked_pnl,
            active_reservations: self
                .reservations
                .values()
                .filter(|value| value.status == ReservationStatus::Active)
                .count(),
            active_locks: self
                .locks
                .values()
                .filter(|value| value.status == LockStatus::Active)
                .count(),
            halted: self.halted.is_some(),
            halt_reason: self.halted.clone(),
            digest: self.state_digest(),
        }
    }

    #[must_use]
    pub const fn accepted_commands(&self) -> u64 {
        self.accepted_commands
    }

    /// Captures confirmed ledger assets plus only the requested command IDs.
    ///
    /// This keeps reconciliation frames bounded by their registered intents
    /// instead of exporting the ledger's complete idempotency history.
    ///
    /// # Panics
    ///
    /// Panics only if private accepted ledger state violates its checked asset
    /// aggregation invariants. Public mutation paths verify those invariants
    /// transactionally before installing state.
    #[must_use]
    pub fn reconciliation_view(
        &self,
        requested_command_ids: &BTreeSet<CommandId>,
    ) -> LedgerReconciliationView {
        let snapshot = self.snapshot();
        let mut token_balances = BTreeMap::new();
        for ((account, asset), balance) in &self.balances {
            if !matches!(
                account,
                Account::TokenAvailable | Account::TokenReserved(_) | Account::LockedToken(_)
            ) {
                continue;
            }
            if let Asset::Outcome(token) = asset {
                let total = token_balances.entry(token.clone()).or_insert(0_i128);
                *total = total
                    .checked_add(*balance)
                    .expect("accepted token balances have checked aggregate arithmetic");
            }
        }
        token_balances.retain(|_, balance| *balance != 0);
        LedgerReconciliationView {
            ledger_digest: snapshot.digest,
            accepted_commands: snapshot.accepted_commands,
            halted: snapshot.halted,
            collateral_micros: snapshot
                .cash_available_micros
                .checked_add(snapshot.cash_reserved_micros)
                .expect("accepted collateral balances have checked aggregate arithmetic"),
            token_balances: token_balances
                .into_iter()
                .map(|(token, balance_micros)| ConfirmedTokenBalance {
                    token,
                    balance_micros,
                })
                .collect(),
            present_command_ids: requested_command_ids
                .iter()
                .filter(|id| self.processed.contains_key(id))
                .copied()
                .collect(),
        }
    }

    /// Captures confirmed assets by accessibility category for pre-trade risk.
    ///
    /// # Panics
    ///
    /// Panics only if private accepted ledger state violates checked aggregate
    /// arithmetic. Public mutation paths verify those invariants before install.
    #[must_use]
    pub fn risk_view(&self) -> LedgerRiskView {
        let snapshot = self.snapshot();
        LedgerRiskView {
            ledger_digest: snapshot.digest,
            halted: snapshot.halted,
            cash_available_micros: snapshot.cash_available_micros,
            cash_reserved_micros: snapshot.cash_reserved_micros,
            available_tokens: self
                .token_category(|account| matches!(account, Account::TokenAvailable)),
            reserved_tokens: self
                .token_category(|account| matches!(account, Account::TokenReserved(_))),
            locked_tokens: self
                .token_category(|account| matches!(account, Account::LockedToken(_))),
        }
    }

    fn token_category(&self, include: impl Fn(&Account) -> bool) -> Vec<ConfirmedTokenBalance> {
        let mut balances = BTreeMap::new();
        for ((account, asset), balance) in &self.balances {
            if !include(account) {
                continue;
            }
            if let Asset::Outcome(token) = asset {
                let total = balances.entry(token.clone()).or_insert(0_i128);
                *total = total
                    .checked_add(*balance)
                    .expect("accepted token category has checked aggregate arithmetic");
            }
        }
        balances
            .into_iter()
            .filter(|(_, balance)| *balance != 0)
            .map(|(token, balance_micros)| ConfirmedTokenBalance {
                token,
                balance_micros,
            })
            .collect()
    }

    fn apply_fresh(&mut self, command: &LedgerCommand) -> Result<(), Error> {
        validate_command(command)?;
        match command {
            LedgerCommand::FundCollateral { amount_micros, .. } => self.post(&Transaction {
                postings: vec![
                    posting(Account::CashAvailable, Asset::Collateral, *amount_micros),
                    posting(
                        Account::CapitalContributed,
                        Asset::Collateral,
                        checked_neg(*amount_micros)?,
                    ),
                ],
            }),
            LedgerCommand::ReserveCollateral {
                reservation_id,
                amount_micros,
                ..
            } => self.reserve(
                *reservation_id,
                ReservationAsset::Collateral,
                *amount_micros,
            ),
            LedgerCommand::ReserveToken {
                reservation_id,
                token,
                quantity_micros,
                ..
            } => self.reserve(
                *reservation_id,
                ReservationAsset::Token(token.clone()),
                *quantity_micros,
            ),
            LedgerCommand::ReleaseReservation { reservation_id, .. } => {
                self.release(*reservation_id)
            }
            LedgerCommand::ConfirmBuy {
                reservation_id,
                token,
                quantity_micros,
                consideration_micros,
                fee_micros,
                ..
            } => self.confirm_buy(
                *reservation_id,
                token,
                *quantity_micros,
                *consideration_micros,
                *fee_micros,
            ),
            LedgerCommand::ConfirmSell {
                reservation_id,
                quantity_micros,
                gross_proceeds_micros,
                fee_micros,
                ..
            } => self.confirm_sell(
                *reservation_id,
                *quantity_micros,
                *gross_proceeds_micros,
                *fee_micros,
            ),
            LedgerCommand::LockPair {
                lock_id,
                up,
                down,
                quantity_micros,
                ..
            } => self.lock_pair(*lock_id, up, down, *quantity_micros),
            LedgerCommand::ConfirmMerge { lock_id, .. } => self.confirm_merge(*lock_id),
            LedgerCommand::ConfirmSplit {
                reservation_id,
                up,
                down,
                quantity_micros,
                ..
            } => self.confirm_split(*reservation_id, up, down, *quantity_micros),
            LedgerCommand::ConfirmRedemption {
                reservation_id,
                quantity_micros,
                payout_micros,
                ..
            } => self.confirm_redemption(*reservation_id, *quantity_micros, *payout_micros),
        }
    }

    fn reserve(
        &mut self,
        id: ReservationId,
        asset: ReservationAsset,
        amount: i128,
    ) -> Result<(), Error> {
        if self.reservations.contains_key(&id) {
            return Err(Error::ReservationExists);
        }
        let transaction = match &asset {
            ReservationAsset::Collateral => Transaction {
                postings: vec![
                    posting(Account::CashAvailable, Asset::Collateral, -amount),
                    posting(Account::CashReserved(id), Asset::Collateral, amount),
                ],
            },
            ReservationAsset::Token(token) => {
                let token_asset = Asset::Outcome(token.clone());
                Transaction {
                    postings: vec![
                        posting(Account::TokenAvailable, token_asset.clone(), -amount),
                        posting(Account::TokenReserved(id), token_asset, amount),
                    ],
                }
            }
        };
        self.post(&transaction)?;
        self.reservations.insert(
            id,
            Reservation {
                id,
                asset,
                original_micros: amount,
                remaining_micros: amount,
                status: ReservationStatus::Active,
            },
        );
        Ok(())
    }

    fn release(&mut self, id: ReservationId) -> Result<(), Error> {
        let reservation = self
            .reservations
            .get(&id)
            .cloned()
            .ok_or(Error::ReservationMissing)?;
        if reservation.status != ReservationStatus::Active {
            return Err(Error::ReservationInactive);
        }
        let amount = reservation.remaining_micros;
        let transaction = match &reservation.asset {
            ReservationAsset::Collateral => Transaction {
                postings: vec![
                    posting(Account::CashReserved(id), Asset::Collateral, -amount),
                    posting(Account::CashAvailable, Asset::Collateral, amount),
                ],
            },
            ReservationAsset::Token(token) => {
                let asset = Asset::Outcome(token.clone());
                Transaction {
                    postings: vec![
                        posting(Account::TokenReserved(id), asset.clone(), -amount),
                        posting(Account::TokenAvailable, asset, amount),
                    ],
                }
            }
        };
        self.post(&transaction)?;
        let mutable = self
            .reservations
            .get_mut(&id)
            .ok_or(Error::ReservationMissing)?;
        mutable.remaining_micros = 0;
        mutable.status = ReservationStatus::Released;
        Ok(())
    }

    fn confirm_buy(
        &mut self,
        id: ReservationId,
        token: &TokenKey,
        quantity: i128,
        consideration: i128,
        fee: i128,
    ) -> Result<(), Error> {
        let total = consideration.checked_add(fee).ok_or(Error::Overflow)?;
        let reservation = self.active_reservation(id)?;
        if reservation.asset != ReservationAsset::Collateral {
            return Err(Error::ReservationMismatch);
        }
        if reservation.remaining_micros < total {
            return Err(Error::ReservationInsufficient);
        }
        self.post(&Transaction {
            postings: vec![
                posting(Account::CashReserved(id), Asset::Collateral, -total),
                posting(
                    Account::InventoryCost(token.clone()),
                    Asset::Collateral,
                    consideration,
                ),
                posting(Account::FeeExpense, Asset::Collateral, fee),
                posting(
                    Account::TokenAvailable,
                    Asset::Outcome(token.clone()),
                    quantity,
                ),
                posting(Account::External, Asset::Outcome(token.clone()), -quantity),
            ],
        })?;
        self.consume_reservation(id, total)?;
        let position = self.costs.entry(token.clone()).or_insert(CostPosition {
            quantity_micros: 0,
            cost_micros: 0,
        });
        position.quantity_micros = position
            .quantity_micros
            .checked_add(quantity)
            .ok_or(Error::Overflow)?;
        position.cost_micros = position
            .cost_micros
            .checked_add(consideration)
            .ok_or(Error::Overflow)?;
        Ok(())
    }

    fn confirm_sell(
        &mut self,
        id: ReservationId,
        quantity: i128,
        gross: i128,
        fee: i128,
    ) -> Result<(), Error> {
        let reservation = self.active_reservation(id)?;
        let ReservationAsset::Token(token) = reservation.asset.clone() else {
            return Err(Error::ReservationMismatch);
        };
        if reservation.remaining_micros < quantity {
            return Err(Error::ReservationInsufficient);
        }
        let position = self.cost_position(&token);
        let allocated_cost = allocate_cost(position, quantity)?;
        let net = gross.checked_sub(fee).ok_or(Error::Overflow)?;
        self.post(&Transaction {
            postings: vec![
                posting(
                    Account::TokenReserved(id),
                    Asset::Outcome(token.clone()),
                    -quantity,
                ),
                posting(Account::External, Asset::Outcome(token.clone()), quantity),
                posting(Account::CashAvailable, Asset::Collateral, net),
                posting(Account::FeeExpense, Asset::Collateral, fee),
                posting(Account::TradingRevenue, Asset::Collateral, -gross),
                posting(
                    Account::InventoryCost(token.clone()),
                    Asset::Collateral,
                    -allocated_cost,
                ),
                posting(Account::CostOfGoodsSold, Asset::Collateral, allocated_cost),
            ],
        })?;
        self.consume_reservation(id, quantity)?;
        self.reduce_cost(&token, quantity, allocated_cost)?;
        Ok(())
    }

    fn lock_pair(
        &mut self,
        id: LockId,
        up: &TokenKey,
        down: &TokenKey,
        quantity: i128,
    ) -> Result<(), Error> {
        if self.locks.contains_key(&id) {
            return Err(Error::LockExists);
        }
        if up == down || up.condition_id != down.condition_id {
            return Err(Error::PairIdentity);
        }
        let up_position = self.cost_position(up);
        let down_position = self.cost_position(down);
        let up_cost = allocate_cost(up_position, quantity)?;
        let down_cost = allocate_cost(down_position, quantity)?;
        let total_cost = up_cost.checked_add(down_cost).ok_or(Error::Overflow)?;
        let payout = quantity;
        self.post(&Transaction {
            postings: vec![
                posting(
                    Account::TokenAvailable,
                    Asset::Outcome(up.clone()),
                    -quantity,
                ),
                posting(
                    Account::LockedToken(id),
                    Asset::Outcome(up.clone()),
                    quantity,
                ),
                posting(
                    Account::TokenAvailable,
                    Asset::Outcome(down.clone()),
                    -quantity,
                ),
                posting(
                    Account::LockedToken(id),
                    Asset::Outcome(down.clone()),
                    quantity,
                ),
                posting(
                    Account::InventoryCost(up.clone()),
                    Asset::Collateral,
                    -up_cost,
                ),
                posting(
                    Account::InventoryCost(down.clone()),
                    Asset::Collateral,
                    -down_cost,
                ),
                posting(Account::LockedCost(id), Asset::Collateral, total_cost),
            ],
        })?;
        self.reduce_cost(up, quantity, up_cost)?;
        self.reduce_cost(down, quantity, down_cost)?;
        self.locks.insert(
            id,
            PairLock {
                id,
                up: up.clone(),
                down: down.clone(),
                quantity_micros: quantity,
                cost_micros: total_cost,
                payout_micros: payout,
                status: LockStatus::Active,
            },
        );
        Ok(())
    }

    fn confirm_merge(&mut self, id: LockId) -> Result<(), Error> {
        let lock = self.locks.get(&id).cloned().ok_or(Error::LockMissing)?;
        if lock.status != LockStatus::Active {
            return Err(Error::LockInactive);
        }
        self.post(&Transaction {
            postings: vec![
                posting(
                    Account::LockedToken(id),
                    Asset::Outcome(lock.up.clone()),
                    -lock.quantity_micros,
                ),
                posting(
                    Account::External,
                    Asset::Outcome(lock.up.clone()),
                    lock.quantity_micros,
                ),
                posting(
                    Account::LockedToken(id),
                    Asset::Outcome(lock.down.clone()),
                    -lock.quantity_micros,
                ),
                posting(
                    Account::External,
                    Asset::Outcome(lock.down.clone()),
                    lock.quantity_micros,
                ),
                posting(
                    Account::CashAvailable,
                    Asset::Collateral,
                    lock.payout_micros,
                ),
                posting(
                    Account::TradingRevenue,
                    Asset::Collateral,
                    -lock.payout_micros,
                ),
                posting(
                    Account::LockedCost(id),
                    Asset::Collateral,
                    -lock.cost_micros,
                ),
                posting(
                    Account::CostOfGoodsSold,
                    Asset::Collateral,
                    lock.cost_micros,
                ),
            ],
        })?;
        self.locks.get_mut(&id).ok_or(Error::LockMissing)?.status = LockStatus::Merged;
        Ok(())
    }

    fn confirm_split(
        &mut self,
        id: ReservationId,
        up: &TokenKey,
        down: &TokenKey,
        quantity: i128,
    ) -> Result<(), Error> {
        if up == down || up.condition_id != down.condition_id {
            return Err(Error::PairIdentity);
        }
        let reservation = self.active_reservation(id)?;
        if reservation.asset != ReservationAsset::Collateral
            || reservation.remaining_micros < quantity
        {
            return Err(Error::ReservationMismatch);
        }
        let up_cost = quantity / 2;
        let down_cost = quantity.checked_sub(up_cost).ok_or(Error::Overflow)?;
        self.post(&Transaction {
            postings: vec![
                posting(Account::CashReserved(id), Asset::Collateral, -quantity),
                posting(
                    Account::InventoryCost(up.clone()),
                    Asset::Collateral,
                    up_cost,
                ),
                posting(
                    Account::InventoryCost(down.clone()),
                    Asset::Collateral,
                    down_cost,
                ),
                posting(
                    Account::TokenAvailable,
                    Asset::Outcome(up.clone()),
                    quantity,
                ),
                posting(Account::External, Asset::Outcome(up.clone()), -quantity),
                posting(
                    Account::TokenAvailable,
                    Asset::Outcome(down.clone()),
                    quantity,
                ),
                posting(Account::External, Asset::Outcome(down.clone()), -quantity),
            ],
        })?;
        self.consume_reservation(id, quantity)?;
        self.add_cost(up, quantity, up_cost)?;
        self.add_cost(down, quantity, down_cost)
    }

    fn confirm_redemption(
        &mut self,
        id: ReservationId,
        quantity: i128,
        payout: i128,
    ) -> Result<(), Error> {
        let reservation = self.active_reservation(id)?;
        let ReservationAsset::Token(token) = reservation.asset.clone() else {
            return Err(Error::ReservationMismatch);
        };
        if reservation.remaining_micros < quantity || payout > quantity {
            return Err(Error::ReservationInsufficient);
        }
        let position = self.cost_position(&token);
        let allocated_cost = allocate_cost(position, quantity)?;
        self.post(&Transaction {
            postings: vec![
                posting(
                    Account::TokenReserved(id),
                    Asset::Outcome(token.clone()),
                    -quantity,
                ),
                posting(Account::External, Asset::Outcome(token.clone()), quantity),
                posting(Account::CashAvailable, Asset::Collateral, payout),
                posting(Account::TradingRevenue, Asset::Collateral, -payout),
                posting(
                    Account::InventoryCost(token.clone()),
                    Asset::Collateral,
                    -allocated_cost,
                ),
                posting(Account::CostOfGoodsSold, Asset::Collateral, allocated_cost),
            ],
        })?;
        self.consume_reservation(id, quantity)?;
        self.reduce_cost(&token, quantity, allocated_cost)
    }

    fn add_cost(&mut self, token: &TokenKey, quantity: i128, cost: i128) -> Result<(), Error> {
        let position = self.costs.entry(token.clone()).or_insert(CostPosition {
            quantity_micros: 0,
            cost_micros: 0,
        });
        position.quantity_micros = position
            .quantity_micros
            .checked_add(quantity)
            .ok_or(Error::Overflow)?;
        position.cost_micros = position
            .cost_micros
            .checked_add(cost)
            .ok_or(Error::Overflow)?;
        Ok(())
    }

    fn active_reservation(&self, id: ReservationId) -> Result<&Reservation, Error> {
        let reservation = self
            .reservations
            .get(&id)
            .ok_or(Error::ReservationMissing)?;
        if reservation.status != ReservationStatus::Active {
            return Err(Error::ReservationInactive);
        }
        Ok(reservation)
    }

    fn consume_reservation(&mut self, id: ReservationId, amount: i128) -> Result<(), Error> {
        let reservation = self
            .reservations
            .get_mut(&id)
            .ok_or(Error::ReservationMissing)?;
        reservation.remaining_micros = reservation
            .remaining_micros
            .checked_sub(amount)
            .ok_or(Error::Overflow)?;
        if reservation.remaining_micros == 0 {
            reservation.status = ReservationStatus::Consumed;
        }
        Ok(())
    }

    fn reduce_cost(&mut self, token: &TokenKey, quantity: i128, cost: i128) -> Result<(), Error> {
        let position = self
            .costs
            .get_mut(token)
            .ok_or(Error::InventoryInsufficient)?;
        position.quantity_micros = position
            .quantity_micros
            .checked_sub(quantity)
            .ok_or(Error::Overflow)?;
        position.cost_micros = position
            .cost_micros
            .checked_sub(cost)
            .ok_or(Error::Overflow)?;
        if position.quantity_micros == 0 && position.cost_micros != 0 {
            return Err(Error::InventoryInvariant);
        }
        Ok(())
    }

    fn post(&mut self, transaction: &Transaction) -> Result<(), Error> {
        let nonzero = transaction
            .postings
            .iter()
            .filter(|posting| posting.delta_micros != 0)
            .count();
        if nonzero < 2 {
            return Err(Error::PostingCount);
        }
        let mut totals: BTreeMap<Asset, i128> = BTreeMap::new();
        let mut candidate = self.balances.clone();
        for entry in transaction
            .postings
            .iter()
            .filter(|entry| entry.delta_micros != 0)
        {
            validate_account_asset(&entry.account, &entry.asset)?;
            let total = totals.entry(entry.asset.clone()).or_default();
            *total = total
                .checked_add(entry.delta_micros)
                .ok_or(Error::Overflow)?;
            let balance = candidate
                .entry((entry.account.clone(), entry.asset.clone()))
                .or_default();
            *balance = balance
                .checked_add(entry.delta_micros)
                .ok_or(Error::Overflow)?;
            if requires_nonnegative(&entry.account) && *balance < 0 {
                return Err(Error::NegativeBalance);
            }
        }
        if let Some((asset, _)) = totals.into_iter().find(|(_, total)| *total != 0) {
            return Err(Error::Unbalanced(asset));
        }
        candidate.retain(|_, balance| *balance != 0);
        self.balances = candidate;
        Ok(())
    }

    fn verify_invariants(&self) -> Result<(), Error> {
        let mut totals: BTreeMap<Asset, i128> = BTreeMap::new();
        for ((account, asset), balance) in &self.balances {
            validate_account_asset(account, asset)?;
            if requires_nonnegative(account) && *balance < 0 {
                return Err(Error::NegativeBalance);
            }
            let total = totals.entry(asset.clone()).or_default();
            *total = total.checked_add(*balance).ok_or(Error::Overflow)?;
        }
        if totals.values().any(|total| *total != 0) {
            return Err(Error::Conservation);
        }

        for reservation in self.reservations.values() {
            let expected = if reservation.status == ReservationStatus::Active {
                reservation.remaining_micros
            } else {
                0
            };
            let actual = match &reservation.asset {
                ReservationAsset::Collateral => {
                    self.balance(&Account::CashReserved(reservation.id), &Asset::Collateral)
                }
                ReservationAsset::Token(token) => self.balance(
                    &Account::TokenReserved(reservation.id),
                    &Asset::Outcome(token.clone()),
                ),
            };
            if expected != actual
                || reservation.original_micros <= 0
                || reservation.remaining_micros < 0
                || reservation.remaining_micros > reservation.original_micros
            {
                return Err(Error::ReservationInvariant);
            }
        }

        let mut tokens = BTreeSet::new();
        tokens.extend(self.costs.keys().cloned());
        for reservation in self.reservations.values() {
            if let ReservationAsset::Token(token) = &reservation.asset {
                tokens.insert(token.clone());
            }
        }
        for token in tokens {
            let available = self.balance(&Account::TokenAvailable, &Asset::Outcome(token.clone()));
            let reserved: i128 = self
                .reservations
                .values()
                .filter_map(|reservation| match &reservation.asset {
                    ReservationAsset::Token(value)
                        if *value == token && reservation.status == ReservationStatus::Active =>
                    {
                        Some(reservation.remaining_micros)
                    }
                    _ => None,
                })
                .sum();
            let position = self.cost_position(&token);
            if position.quantity_micros != available + reserved
                || position.quantity_micros < 0
                || position.cost_micros < 0
                || self.balance(&Account::InventoryCost(token.clone()), &Asset::Collateral)
                    != position.cost_micros
            {
                return Err(Error::InventoryInvariant);
            }
        }
        for lock in self.locks.values() {
            let expected = if lock.status == LockStatus::Active {
                lock.quantity_micros
            } else {
                0
            };
            for token in [&lock.up, &lock.down] {
                if self.balance(
                    &Account::LockedToken(lock.id),
                    &Asset::Outcome(token.clone()),
                ) != expected
                {
                    return Err(Error::InventoryInvariant);
                }
            }
            let cost = if lock.status == LockStatus::Active {
                lock.cost_micros
            } else {
                0
            };
            if self.balance(&Account::LockedCost(lock.id), &Asset::Collateral) != cost {
                return Err(Error::InventoryInvariant);
            }
        }
        self.checked_pnl()?;
        Ok(())
    }

    fn checked_pnl(&self) -> Result<(i128, i128, i128), Error> {
        let fees = self.balance(&Account::FeeExpense, &Asset::Collateral);
        let revenue = self.balance(&Account::TradingRevenue, &Asset::Collateral);
        let cost_of_goods = self.balance(&Account::CostOfGoodsSold, &Asset::Collateral);
        let realized = revenue
            .checked_neg()
            .and_then(|value| value.checked_sub(cost_of_goods))
            .and_then(|value| value.checked_sub(fees))
            .ok_or(Error::Overflow)?;
        let locked = self
            .locks
            .values()
            .filter(|lock| lock.status == LockStatus::Active)
            .try_fold(0_i128, |total, lock| {
                lock.payout_micros
                    .checked_sub(lock.cost_micros)
                    .and_then(|value| total.checked_add(value))
                    .ok_or(Error::Overflow)
            })?;
        Ok((fees, realized, locked))
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"accounting-ledger-state-v1");
        hasher.update(&self.accepted_commands.to_le_bytes());
        for ((account, asset), balance) in &self.balances {
            hash_json(&mut hasher, account);
            hash_json(&mut hasher, asset);
            hasher.update(&balance.to_le_bytes());
        }
        for (id, reservation) in &self.reservations {
            hasher.update(&id.0);
            hash_json(&mut hasher, reservation);
        }
        for (token, position) in &self.costs {
            hash_json(&mut hasher, token);
            hash_json(&mut hasher, position);
        }
        for (id, lock) in &self.locks {
            hasher.update(&id.0);
            hash_json(&mut hasher, lock);
        }
        for (id, digest) in &self.processed {
            hasher.update(&id.0);
            hasher.update(digest);
        }
        if let Some(reason) = &self.halted {
            hasher.update(reason.as_bytes());
        }
        *hasher.finalize().as_bytes()
    }
}

fn posting(account: Account, asset: Asset, delta_micros: i128) -> Posting {
    Posting {
        account,
        asset,
        delta_micros,
    }
}

fn checked_neg(value: i128) -> Result<i128, Error> {
    value.checked_neg().ok_or(Error::Overflow)
}

fn allocate_cost(position: CostPosition, quantity: i128) -> Result<i128, Error> {
    if quantity <= 0
        || position.quantity_micros < quantity
        || position.quantity_micros <= 0
        || position.cost_micros < 0
    {
        return Err(Error::InventoryInsufficient);
    }
    if quantity == position.quantity_micros {
        return Ok(position.cost_micros);
    }
    let product = position
        .cost_micros
        .checked_mul(quantity)
        .ok_or(Error::Overflow)?;
    product
        .checked_add(position.quantity_micros - 1)
        .ok_or(Error::Overflow)
        .map(|value| value / position.quantity_micros)
}

fn validate_account_asset(account: &Account, asset: &Asset) -> Result<(), Error> {
    let compatible = match account {
        Account::CashAvailable
        | Account::CashReserved(_)
        | Account::InventoryCost(_)
        | Account::LockedCost(_)
        | Account::FeeExpense
        | Account::TradingRevenue
        | Account::CostOfGoodsSold
        | Account::CapitalContributed => matches!(asset, Asset::Collateral),
        Account::TokenAvailable | Account::TokenReserved(_) | Account::LockedToken(_) => {
            matches!(asset, Asset::Outcome(_))
        }
        Account::External => true,
    };
    match (account, asset) {
        (Account::InventoryCost(account_token), Asset::Outcome(asset_token))
            if account_token != asset_token =>
        {
            return Err(Error::AccountAsset);
        }
        _ => {}
    }
    if compatible {
        Ok(())
    } else {
        Err(Error::AccountAsset)
    }
}

fn requires_nonnegative(account: &Account) -> bool {
    matches!(
        account,
        Account::CashAvailable
            | Account::CashReserved(_)
            | Account::TokenAvailable
            | Account::TokenReserved(_)
            | Account::InventoryCost(_)
            | Account::LockedToken(_)
            | Account::LockedCost(_)
            | Account::FeeExpense
            | Account::CostOfGoodsSold
    )
}

fn validate_confirmation(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > MAX_CONFIRMATION_BYTES {
        Err(Error::Confirmation)
    } else {
        Ok(())
    }
}

fn validate_command(command: &LedgerCommand) -> Result<(), Error> {
    if command.recorded_at_ns() < 0 {
        return Err(Error::Timestamp);
    }
    match command {
        LedgerCommand::FundCollateral { amount_micros, .. }
        | LedgerCommand::ReserveCollateral { amount_micros, .. } => positive(*amount_micros),
        LedgerCommand::ReserveToken {
            token,
            quantity_micros,
            ..
        } => {
            token.validate()?;
            positive(*quantity_micros)
        }
        LedgerCommand::ReleaseReservation { .. } => Ok(()),
        LedgerCommand::ConfirmBuy {
            token,
            quantity_micros,
            consideration_micros,
            fee_micros,
            confirmation,
            ..
        } => {
            token.validate()?;
            positive(*quantity_micros)?;
            positive(*consideration_micros)?;
            if *fee_micros < 0 {
                return Err(Error::Fee);
            }
            validate_confirmation(confirmation)
        }
        LedgerCommand::ConfirmSell {
            quantity_micros,
            gross_proceeds_micros,
            fee_micros,
            confirmation,
            ..
        } => {
            positive(*quantity_micros)?;
            positive(*gross_proceeds_micros)?;
            if *fee_micros < 0 || fee_micros > gross_proceeds_micros {
                return Err(Error::Fee);
            }
            validate_confirmation(confirmation)
        }
        LedgerCommand::LockPair {
            up,
            down,
            quantity_micros,
            ..
        } => {
            up.validate()?;
            down.validate()?;
            positive(*quantity_micros)?;
            if up == down || up.condition_id != down.condition_id {
                return Err(Error::PairIdentity);
            }
            Ok(())
        }
        LedgerCommand::ConfirmMerge { confirmation, .. } => validate_confirmation(confirmation),
        LedgerCommand::ConfirmSplit {
            up,
            down,
            quantity_micros,
            confirmation,
            ..
        } => {
            up.validate()?;
            down.validate()?;
            positive(*quantity_micros)?;
            if up == down || up.condition_id != down.condition_id {
                return Err(Error::PairIdentity);
            }
            validate_confirmation(confirmation)
        }
        LedgerCommand::ConfirmRedemption {
            quantity_micros,
            payout_micros,
            confirmation,
            ..
        } => {
            positive(*quantity_micros)?;
            if *payout_micros < 0 || payout_micros > quantity_micros {
                return Err(Error::NonPositive);
            }
            validate_confirmation(confirmation)
        }
    }
}

fn positive(value: i128) -> Result<(), Error> {
    if value > 0 {
        Ok(())
    } else {
        Err(Error::NonPositive)
    }
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("internal serializable accounting state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

/// Encodes one exact, versioned command.
///
/// # Errors
///
/// Rejects invalid data or payloads exceeding the hard bound.
pub fn encode_command(command: &LedgerCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one exact versioned command and revalidates all boundaries.
///
/// # Errors
///
/// Rejects malformed JSON, unsupported versions, trailing bytes, and invalid
/// command fields.
pub fn decode_command(bytes: &[u8]) -> Result<LedgerCommand, Error> {
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
