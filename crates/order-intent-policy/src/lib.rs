#![forbid(unsafe_code)]

//! Offline deterministic exchange-mode and order-intent policy authority.
//!
//! A permit is an audit fact only. This crate contains no credential, private
//! key, signature, authenticated client, network transport, or order submission.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurablePolicyEngine,
    PolicyCheckpoint, PolicyRecovery, StorageError,
};

use accounting_ledger::TokenKey;
use portfolio_risk::{
    order_exposure_digest, DecisionReason as RiskReason, DecisionStatus as RiskStatus,
    OrderExposure, RiskDecision, RiskOrderId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 256 * 1024;
const MAX_TEXT_BYTES: usize = 512;
const MAX_ALLOWED_TOKENS: usize = 64;
const MICROS_PER_UNIT: i128 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PolicyCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeMode {
    Unknown,
    Normal,
    Restarting,
    PostOnly,
    CancelOnly,
    TradingDisabled,
    Recovering,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeModeObservation {
    pub sequence: u64,
    pub mode: ExchangeMode,
    pub observed_at_ns: i64,
    pub valid_until_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeInForce {
    Gtc,
    Gtd { expires_at_ns: i64 },
    Fok,
    Fak,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignerPolicyFrame {
    pub policy_id: [u8; 32],
    pub venue: String,
    pub exchange_contract: String,
    pub allowed_tokens: Vec<TokenKey>,
    pub max_quantity_micros: i128,
    pub max_price_micros: i64,
    pub max_notional_micros: i128,
    pub allow_maker: bool,
    pub allow_taker: bool,
    pub valid_from_ns: i64,
    pub valid_until_ns: i64,
}

impl SignerPolicyFrame {
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"order-intent-signer-policy-v1");
        hash_json(&mut hasher, self);
        *hasher.finalize().as_bytes()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementRequest {
    pub approval: RiskDecision,
    pub order: OrderExposure,
    pub venue: String,
    pub exchange_contract: String,
    pub post_only: bool,
    pub marketable: bool,
    pub time_in_force: TimeInForce,
    pub signer_policy: SignerPolicyFrame,
    pub max_approval_age_ns: i64,
    pub max_mode_age_ns: i64,
    pub authorization_expires_at_ns: i64,
    pub evaluated_at_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRequest {
    pub order_id: RiskOrderId,
    pub max_mode_age_ns: i64,
    pub evaluated_at_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PolicyCommand {
    ObserveMode {
        command_id: PolicyCommandId,
        observation: ExchangeModeObservation,
        recorded_at_ns: i64,
    },
    AuthorizePlacement {
        command_id: PolicyCommandId,
        request: Box<PlacementRequest>,
        recorded_at_ns: i64,
    },
    MarkDelayed {
        command_id: PolicyCommandId,
        order_id: RiskOrderId,
        release_at_ns: i64,
        uncancellable_until_ns: i64,
        recorded_at_ns: i64,
    },
    MarkLive {
        command_id: PolicyCommandId,
        order_id: RiskOrderId,
        recorded_at_ns: i64,
    },
    AuthorizeCancel {
        command_id: PolicyCommandId,
        request: CancelRequest,
        recorded_at_ns: i64,
    },
    MarkTerminal {
        command_id: PolicyCommandId,
        order_id: RiskOrderId,
        recorded_at_ns: i64,
    },
}

impl PolicyCommand {
    #[must_use]
    pub const fn command_id(&self) -> PolicyCommandId {
        match self {
            Self::ObserveMode { command_id, .. }
            | Self::AuthorizePlacement { command_id, .. }
            | Self::MarkDelayed { command_id, .. }
            | Self::MarkLive { command_id, .. }
            | Self::AuthorizeCancel { command_id, .. }
            | Self::MarkTerminal { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::ObserveMode { recorded_at_ns, .. }
            | Self::AuthorizePlacement { recorded_at_ns, .. }
            | Self::MarkDelayed { recorded_at_ns, .. }
            | Self::MarkLive { recorded_at_ns, .. }
            | Self::AuthorizeCancel { recorded_at_ns, .. }
            | Self::MarkTerminal { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: PolicyCommand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyStatus {
    Permit,
    Deny,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    ObserveMode,
    Place,
    Lifecycle,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyReason {
    ModeAccepted,
    PlacementPermitted,
    LifecycleAccepted,
    CancelPermitted,
    ModeUnavailable,
    ModeStale,
    ModeForbidsPlacement,
    ModeForbidsCancel,
    PostOnlyViolation,
    RiskNotApproved,
    RiskDigestInvalid,
    RiskOrderMismatch,
    RiskApprovalStale,
    ApprovalAlreadyUsed,
    AuthorizationExpired,
    SignerPolicyInvalid,
    SignerPolicyInactive,
    VenueForbidden,
    ContractForbidden,
    TokenForbidden,
    QuantityLimit,
    PriceLimit,
    NotionalLimit,
    MakerForbidden,
    TakerForbidden,
    TimeInForceInvalid,
    OrderAlreadyTracked,
    OrderUnknown,
    OrderTerminal,
    CancelAlreadyAuthorized,
    Uncancellable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDecision {
    pub command_id: PolicyCommandId,
    pub action: PolicyAction,
    pub status: PolicyStatus,
    pub reason: PolicyReason,
    pub order_id: Option<RiskOrderId>,
    pub exchange_mode: ExchangeMode,
    pub risk_decision_digest: Option<[u8; 32]>,
    pub signer_policy_digest: Option<[u8; 32]>,
    pub subject_digest: Option<[u8; 32]>,
    pub decision_digest: [u8; 32],
}

impl PolicyDecision {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == decision_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStage {
    Authorized,
    Delayed {
        release_at_ns: i64,
        uncancellable_until_ns: i64,
    },
    Live,
    CancelAuthorized,
    Terminal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackedOrder {
    pub order: OrderExposure,
    pub approval_digest: [u8; 32],
    pub signer_policy_digest: [u8; 32],
    pub authorization_expires_at_ns: i64,
    pub stage: OrderStage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicySnapshot {
    pub accepted_commands: u64,
    pub mode: ExchangeMode,
    pub mode_sequence: Option<u64>,
    pub mode_observed_at_ns: Option<i64>,
    pub tracked_orders: usize,
    pub used_approvals: usize,
    pub last_recorded_at_ns: Option<i64>,
    pub last_decision: Option<PolicyDecision>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("policy command timestamp is invalid")]
    Timestamp,
    #[error("policy command exceeds its canonical bound")]
    CommandBound,
    #[error("policy command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported policy command version: {0}")]
    Version(u16),
    #[error("policy command id was reused for different content")]
    IdempotencyConflict,
    #[error("exchange-mode history regressed or equivocated")]
    ModeHistory,
    #[error("policy command time regressed")]
    ClockRegression,
    #[error("order lifecycle transition is impossible")]
    LifecycleTransition,
    #[error("policy arithmetic overflow")]
    Overflow,
    #[error("policy engine is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug, Default)]
pub struct IntentPolicyEngine {
    processed: BTreeMap<PolicyCommandId, ([u8; 32], PolicyDecision)>,
    mode: Option<ExchangeModeObservation>,
    orders: BTreeMap<RiskOrderId, TrackedOrder>,
    used_approvals: BTreeSet<[u8; 32]>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    last_decision: Option<PolicyDecision>,
    halted: Option<String>,
}

impl IntentPolicyEngine {
    /// Applies one offline policy command.
    ///
    /// # Errors
    ///
    /// Returns canonical validation or absorbing integrity failures.
    pub fn apply(&mut self, command: &PolicyCommand) -> Result<PolicyDecision, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        let bytes = encode_command(command)?;
        let content = *blake3::hash(&bytes).as_bytes();
        let id = command.command_id();
        if let Some((existing, decision)) = self.processed.get(&id) {
            if *existing == content {
                return Ok(decision.clone());
            }
            return self.install_halt(Error::IdempotencyConflict);
        }
        if self
            .last_recorded_at_ns
            .is_some_and(|previous| command.recorded_at_ns() < previous)
        {
            return self.install_halt(Error::ClockRegression);
        }
        let mut candidate = self.clone();
        let result = match candidate.apply_fresh(command) {
            Ok(value) => value,
            Err(error) => return self.install_halt(error),
        };
        candidate.accepted_commands = match candidate.accepted_commands.checked_add(1) {
            Some(value) => value,
            None => return self.install_halt(Error::Overflow),
        };
        candidate.last_recorded_at_ns = Some(command.recorded_at_ns());
        candidate.last_decision = Some(result.clone());
        candidate.processed.insert(id, (content, result.clone()));
        *self = candidate;
        Ok(result)
    }

    #[must_use]
    pub fn snapshot(&self) -> PolicySnapshot {
        PolicySnapshot {
            accepted_commands: self.accepted_commands,
            mode: self
                .mode
                .as_ref()
                .map_or(ExchangeMode::Unknown, |value| value.mode),
            mode_sequence: self.mode.as_ref().map(|value| value.sequence),
            mode_observed_at_ns: self.mode.as_ref().map(|value| value.observed_at_ns),
            tracked_orders: self.orders.len(),
            used_approvals: self.used_approvals.len(),
            last_recorded_at_ns: self.last_recorded_at_ns,
            last_decision: self.last_decision.clone(),
            halted: self.halted.is_some(),
            halt_reason: self.halted.clone(),
            digest: self.state_digest(),
        }
    }

    #[must_use]
    pub fn order(&self, id: RiskOrderId) -> Option<&TrackedOrder> {
        self.orders.get(&id)
    }

    #[must_use]
    pub const fn is_halted(&self) -> bool {
        self.halted.is_some()
    }

    fn apply_fresh(&mut self, command: &PolicyCommand) -> Result<PolicyDecision, Error> {
        match command {
            PolicyCommand::ObserveMode {
                command_id,
                observation,
                ..
            } => self.observe_mode(*command_id, observation),
            PolicyCommand::AuthorizePlacement {
                command_id,
                request,
                ..
            } => self.authorize_placement(*command_id, request),
            PolicyCommand::MarkDelayed {
                command_id,
                order_id,
                release_at_ns,
                uncancellable_until_ns,
                recorded_at_ns,
            } => self.mark_delayed(
                *command_id,
                *order_id,
                *release_at_ns,
                *uncancellable_until_ns,
                *recorded_at_ns,
            ),
            PolicyCommand::MarkLive {
                command_id,
                order_id,
                recorded_at_ns,
            } => self.mark_live(*command_id, *order_id, *recorded_at_ns),
            PolicyCommand::AuthorizeCancel {
                command_id,
                request,
                ..
            } => self.authorize_cancel(*command_id, request),
            PolicyCommand::MarkTerminal {
                command_id,
                order_id,
                ..
            } => self.mark_terminal(*command_id, *order_id),
        }
    }

    fn observe_mode(
        &mut self,
        id: PolicyCommandId,
        observation: &ExchangeModeObservation,
    ) -> Result<PolicyDecision, Error> {
        if observation.observed_at_ns < 0
            || observation.valid_until_ns <= observation.observed_at_ns
        {
            return Err(Error::Timestamp);
        }
        if let Some(previous) = &self.mode {
            if observation.sequence < previous.sequence
                || observation.observed_at_ns < previous.observed_at_ns
                || (observation.sequence == previous.sequence && observation != previous)
            {
                return Err(Error::ModeHistory);
            }
            if observation.sequence == previous.sequence {
                return Ok(make_decision(
                    id,
                    PolicyAction::ObserveMode,
                    PolicyStatus::Permit,
                    PolicyReason::ModeAccepted,
                    previous.mode,
                    DecisionEvidence::default(),
                ));
            }
        }
        self.mode = Some(observation.clone());
        Ok(make_decision(
            id,
            PolicyAction::ObserveMode,
            PolicyStatus::Permit,
            PolicyReason::ModeAccepted,
            observation.mode,
            DecisionEvidence::default(),
        ))
    }

    fn authorize_placement(
        &mut self,
        id: PolicyCommandId,
        request: &PlacementRequest,
    ) -> Result<PolicyDecision, Error> {
        let mode = self
            .mode
            .as_ref()
            .map_or(ExchangeMode::Unknown, |value| value.mode);
        let signer_digest = request.signer_policy.digest();
        let reason = self.placement_denial(request)?;
        if let Some(reason) = reason {
            return Ok(make_decision(
                id,
                PolicyAction::Place,
                PolicyStatus::Deny,
                reason,
                mode,
                DecisionEvidence {
                    order_id: Some(request.order.order_id),
                    risk_digest: Some(request.approval.decision_digest),
                    signer_digest: Some(signer_digest),
                    subject_digest: Some(placement_request_digest(request)),
                },
            ));
        }
        self.used_approvals.insert(request.approval.decision_digest);
        self.orders.insert(
            request.order.order_id,
            TrackedOrder {
                order: request.order.clone(),
                approval_digest: request.approval.decision_digest,
                signer_policy_digest: signer_digest,
                authorization_expires_at_ns: request.authorization_expires_at_ns,
                stage: OrderStage::Authorized,
            },
        );
        Ok(make_decision(
            id,
            PolicyAction::Place,
            PolicyStatus::Permit,
            PolicyReason::PlacementPermitted,
            mode,
            DecisionEvidence {
                order_id: Some(request.order.order_id),
                risk_digest: Some(request.approval.decision_digest),
                signer_digest: Some(signer_digest),
                subject_digest: Some(placement_request_digest(request)),
            },
        ))
    }

    fn placement_denial(&self, request: &PlacementRequest) -> Result<Option<PolicyReason>, Error> {
        if !request.approval.verify_digest() {
            return Ok(Some(PolicyReason::RiskDigestInvalid));
        }
        if request.approval.status != RiskStatus::Approve
            || request.approval.reason != RiskReason::AllLimitsSatisfied
        {
            return Ok(Some(PolicyReason::RiskNotApproved));
        }
        if request.approval.candidate_order_digest != order_exposure_digest(&request.order) {
            return Ok(Some(PolicyReason::RiskOrderMismatch));
        }
        if request.evaluated_at_ns < request.approval.evaluated_at_ns
            || request.max_approval_age_ns < 0
            || request.evaluated_at_ns - request.approval.evaluated_at_ns
                > request.max_approval_age_ns
        {
            return Ok(Some(PolicyReason::RiskApprovalStale));
        }
        if self
            .used_approvals
            .contains(&request.approval.decision_digest)
        {
            return Ok(Some(PolicyReason::ApprovalAlreadyUsed));
        }
        if self.orders.contains_key(&request.order.order_id) {
            return Ok(Some(PolicyReason::OrderAlreadyTracked));
        }
        if request.authorization_expires_at_ns <= request.evaluated_at_ns {
            return Ok(Some(PolicyReason::AuthorizationExpired));
        }
        if let Some(reason) = mode_place_denial(self.mode.as_ref(), request) {
            return Ok(Some(reason));
        }
        if let Some(reason) = signer_denial(request)? {
            return Ok(Some(reason));
        }
        Ok(time_in_force_denial(request))
    }

    fn mark_delayed(
        &mut self,
        id: PolicyCommandId,
        order_id: RiskOrderId,
        release_at_ns: i64,
        uncancellable_until_ns: i64,
        at: i64,
    ) -> Result<PolicyDecision, Error> {
        if release_at_ns <= at || uncancellable_until_ns < release_at_ns {
            return Err(Error::LifecycleTransition);
        }
        let tracked = self
            .orders
            .get_mut(&order_id)
            .ok_or(Error::LifecycleTransition)?;
        if tracked.stage != OrderStage::Authorized || at >= tracked.authorization_expires_at_ns {
            return Err(Error::LifecycleTransition);
        }
        tracked.stage = OrderStage::Delayed {
            release_at_ns,
            uncancellable_until_ns,
        };
        Ok(self.lifecycle_decision(id, order_id))
    }

    fn mark_live(
        &mut self,
        id: PolicyCommandId,
        order_id: RiskOrderId,
        at: i64,
    ) -> Result<PolicyDecision, Error> {
        let tracked = self
            .orders
            .get_mut(&order_id)
            .ok_or(Error::LifecycleTransition)?;
        match tracked.stage {
            OrderStage::Authorized if at < tracked.authorization_expires_at_ns => {}
            OrderStage::Delayed { release_at_ns, .. } if at >= release_at_ns => {}
            _ => return Err(Error::LifecycleTransition),
        }
        tracked.stage = OrderStage::Live;
        Ok(self.lifecycle_decision(id, order_id))
    }

    fn authorize_cancel(
        &mut self,
        id: PolicyCommandId,
        request: &CancelRequest,
    ) -> Result<PolicyDecision, Error> {
        let mode = self
            .mode
            .as_ref()
            .map_or(ExchangeMode::Unknown, |value| value.mode);
        let Some(tracked) = self.orders.get(&request.order_id) else {
            return Ok(make_decision(
                id,
                PolicyAction::Cancel,
                PolicyStatus::Deny,
                PolicyReason::OrderUnknown,
                mode,
                DecisionEvidence::cancel(request),
            ));
        };
        let reason = if matches!(tracked.stage, OrderStage::Terminal) {
            Some(PolicyReason::OrderTerminal)
        } else if matches!(tracked.stage, OrderStage::CancelAuthorized) {
            Some(PolicyReason::CancelAlreadyAuthorized)
        } else if matches!(tracked.stage, OrderStage::Delayed { uncancellable_until_ns, .. } if request.evaluated_at_ns < uncancellable_until_ns)
        {
            Some(PolicyReason::Uncancellable)
        } else {
            mode_cancel_denial(self.mode.as_ref(), request)
        };
        if let Some(reason) = reason {
            return Ok(make_decision(
                id,
                PolicyAction::Cancel,
                PolicyStatus::Deny,
                reason,
                mode,
                DecisionEvidence::cancel(request),
            ));
        }
        self.orders
            .get_mut(&request.order_id)
            .ok_or(Error::LifecycleTransition)?
            .stage = OrderStage::CancelAuthorized;
        Ok(make_decision(
            id,
            PolicyAction::Cancel,
            PolicyStatus::Permit,
            PolicyReason::CancelPermitted,
            mode,
            DecisionEvidence::cancel(request),
        ))
    }

    fn mark_terminal(
        &mut self,
        id: PolicyCommandId,
        order_id: RiskOrderId,
    ) -> Result<PolicyDecision, Error> {
        let tracked = self
            .orders
            .get_mut(&order_id)
            .ok_or(Error::LifecycleTransition)?;
        if tracked.stage == OrderStage::Terminal {
            return Err(Error::LifecycleTransition);
        }
        tracked.stage = OrderStage::Terminal;
        Ok(self.lifecycle_decision(id, order_id))
    }

    fn lifecycle_decision(&self, id: PolicyCommandId, order_id: RiskOrderId) -> PolicyDecision {
        make_decision(
            id,
            PolicyAction::Lifecycle,
            PolicyStatus::Permit,
            PolicyReason::LifecycleAccepted,
            self.mode
                .as_ref()
                .map_or(ExchangeMode::Unknown, |value| value.mode),
            DecisionEvidence::order(order_id),
        )
    }

    fn install_halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"order-intent-policy-state-v1");
        hash_json(&mut hasher, &self.mode);
        for (id, order) in &self.orders {
            hasher.update(&id.0);
            hash_json(&mut hasher, order);
        }
        hash_json(&mut hasher, &self.used_approvals);
        hash_json(&mut hasher, &self.accepted_commands);
        hash_json(&mut hasher, &self.last_recorded_at_ns);
        hash_json(&mut hasher, &self.last_decision);
        for (id, (content, decision)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_json(&mut hasher, decision);
        }
        hash_json(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }
}

fn mode_place_denial(
    mode: Option<&ExchangeModeObservation>,
    request: &PlacementRequest,
) -> Option<PolicyReason> {
    let Some(mode) = mode else {
        return Some(PolicyReason::ModeUnavailable);
    };
    if request.max_mode_age_ns < 0
        || request.evaluated_at_ns < mode.observed_at_ns
        || request.evaluated_at_ns >= mode.valid_until_ns
        || request.evaluated_at_ns - mode.observed_at_ns > request.max_mode_age_ns
    {
        return Some(PolicyReason::ModeStale);
    }
    match mode.mode {
        ExchangeMode::Normal => None,
        ExchangeMode::PostOnly if request.post_only && !request.marketable => None,
        ExchangeMode::PostOnly => Some(PolicyReason::PostOnlyViolation),
        _ => Some(PolicyReason::ModeForbidsPlacement),
    }
}

fn mode_cancel_denial(
    mode: Option<&ExchangeModeObservation>,
    request: &CancelRequest,
) -> Option<PolicyReason> {
    let Some(mode) = mode else {
        return Some(PolicyReason::ModeUnavailable);
    };
    if request.max_mode_age_ns < 0
        || request.evaluated_at_ns < mode.observed_at_ns
        || request.evaluated_at_ns >= mode.valid_until_ns
        || request.evaluated_at_ns - mode.observed_at_ns > request.max_mode_age_ns
    {
        return Some(PolicyReason::ModeStale);
    }
    match mode.mode {
        ExchangeMode::Normal
        | ExchangeMode::PostOnly
        | ExchangeMode::CancelOnly
        | ExchangeMode::TradingDisabled
        | ExchangeMode::Recovering => None,
        ExchangeMode::Unknown | ExchangeMode::Restarting => Some(PolicyReason::ModeForbidsCancel),
    }
}

fn signer_denial(request: &PlacementRequest) -> Result<Option<PolicyReason>, Error> {
    let policy = &request.signer_policy;
    if !valid_text(&policy.venue)
        || !valid_text(&policy.exchange_contract)
        || policy.allowed_tokens.is_empty()
        || policy.allowed_tokens.len() > MAX_ALLOWED_TOKENS
        || policy.max_quantity_micros <= 0
        || !(0..=1_000_000).contains(&policy.max_price_micros)
        || policy.max_notional_micros <= 0
        || policy.valid_from_ns < 0
        || policy.valid_until_ns <= policy.valid_from_ns
        || !strict_tokens(&policy.allowed_tokens)
    {
        return Ok(Some(PolicyReason::SignerPolicyInvalid));
    }
    if request.evaluated_at_ns < policy.valid_from_ns
        || request.evaluated_at_ns >= policy.valid_until_ns
    {
        return Ok(Some(PolicyReason::SignerPolicyInactive));
    }
    if request.venue != policy.venue {
        return Ok(Some(PolicyReason::VenueForbidden));
    }
    if request.exchange_contract != policy.exchange_contract {
        return Ok(Some(PolicyReason::ContractForbidden));
    }
    if policy
        .allowed_tokens
        .binary_search(&request.order.token)
        .is_err()
    {
        return Ok(Some(PolicyReason::TokenForbidden));
    }
    if request.order.quantity_micros > policy.max_quantity_micros {
        return Ok(Some(PolicyReason::QuantityLimit));
    }
    if request.order.limit_price_micros > policy.max_price_micros {
        return Ok(Some(PolicyReason::PriceLimit));
    }
    let notional = div_ceil(
        i128::from(request.order.limit_price_micros)
            .checked_mul(request.order.quantity_micros)
            .ok_or(Error::Overflow)?,
        MICROS_PER_UNIT,
    )?
    .checked_add(request.order.max_fee_micros)
    .ok_or(Error::Overflow)?;
    if notional > policy.max_notional_micros {
        return Ok(Some(PolicyReason::NotionalLimit));
    }
    if request.marketable && !policy.allow_taker {
        return Ok(Some(PolicyReason::TakerForbidden));
    }
    if !request.marketable && !policy.allow_maker {
        return Ok(Some(PolicyReason::MakerForbidden));
    }
    Ok(None)
}

fn time_in_force_denial(request: &PlacementRequest) -> Option<PolicyReason> {
    if request.post_only && request.marketable {
        return Some(PolicyReason::PostOnlyViolation);
    }
    match request.time_in_force {
        TimeInForce::Gtd { expires_at_ns } if expires_at_ns <= request.evaluated_at_ns => {
            Some(PolicyReason::TimeInForceInvalid)
        }
        TimeInForce::Fok | TimeInForce::Fak if request.post_only => {
            Some(PolicyReason::TimeInForceInvalid)
        }
        _ => None,
    }
}

fn strict_tokens(tokens: &[TokenKey]) -> bool {
    tokens
        .iter()
        .all(|token| valid_text(&token.condition_id) && valid_text(&token.token_id))
        && tokens.windows(2).all(|window| window[0] < window[1])
}

#[derive(Clone, Copy, Default)]
struct DecisionEvidence {
    order_id: Option<RiskOrderId>,
    risk_digest: Option<[u8; 32]>,
    signer_digest: Option<[u8; 32]>,
    subject_digest: Option<[u8; 32]>,
}

impl DecisionEvidence {
    const fn order(order_id: RiskOrderId) -> Self {
        Self {
            order_id: Some(order_id),
            risk_digest: None,
            signer_digest: None,
            subject_digest: None,
        }
    }

    fn cancel(request: &CancelRequest) -> Self {
        Self {
            order_id: Some(request.order_id),
            risk_digest: None,
            signer_digest: None,
            subject_digest: Some(cancel_request_digest(request)),
        }
    }
}

fn make_decision(
    command_id: PolicyCommandId,
    action: PolicyAction,
    status: PolicyStatus,
    reason: PolicyReason,
    exchange_mode: ExchangeMode,
    evidence: DecisionEvidence,
) -> PolicyDecision {
    let mut value = PolicyDecision {
        command_id,
        action,
        status,
        reason,
        order_id: evidence.order_id,
        exchange_mode,
        risk_decision_digest: evidence.risk_digest,
        signer_policy_digest: evidence.signer_digest,
        subject_digest: evidence.subject_digest,
        decision_digest: [0; 32],
    };
    value.decision_digest = decision_digest(&value);
    value
}

fn decision_digest(decision: &PolicyDecision) -> [u8; 32] {
    let mut value = decision.clone();
    value.decision_digest = [0; 32];
    *blake3::hash(&serde_json::to_vec(&value).expect("policy decision serializes")).as_bytes()
}

/// Produces the exact Phase 2.3 fingerprint of a placement request.
#[must_use]
pub fn placement_request_digest(request: &PlacementRequest) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"order-intent-placement-request-v1");
    hash_json(&mut hasher, request);
    *hasher.finalize().as_bytes()
}

/// Produces the exact Phase 2.3 fingerprint of a cancellation request.
#[must_use]
pub fn cancel_request_digest(request: &CancelRequest) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"order-intent-cancel-request-v1");
    hash_json(&mut hasher, request);
    *hasher.finalize().as_bytes()
}

fn div_ceil(value: i128, divisor: i128) -> Result<i128, Error> {
    value
        .checked_add(divisor.checked_sub(1).ok_or(Error::Overflow)?)
        .ok_or(Error::Overflow)
        .map(|adjusted| adjusted / divisor)
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT_BYTES
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("internal policy state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

fn validate_command(command: &PolicyCommand) -> Result<(), Error> {
    if command.recorded_at_ns() < 0 {
        return Err(Error::Timestamp);
    }
    match command {
        PolicyCommand::ObserveMode {
            observation,
            recorded_at_ns,
            ..
        } if observation.observed_at_ns > *recorded_at_ns => Err(Error::Timestamp),
        PolicyCommand::AuthorizePlacement {
            request,
            recorded_at_ns,
            ..
        } if request.evaluated_at_ns != *recorded_at_ns => Err(Error::Timestamp),
        PolicyCommand::AuthorizeCancel {
            request,
            recorded_at_ns,
            ..
        } if request.evaluated_at_ns != *recorded_at_ns => Err(Error::Timestamp),
        _ => Ok(()),
    }
}

/// Encodes one bounded canonical policy command.
///
/// # Errors
///
/// Rejects timestamps and commands beyond the wire bound.
pub fn encode_command(command: &PolicyCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one exact bounded canonical policy command.
///
/// # Errors
///
/// Rejects malformed, trailing, oversized, unsupported, or invalid input.
pub fn decode_command(bytes: &[u8]) -> Result<PolicyCommand, Error> {
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
