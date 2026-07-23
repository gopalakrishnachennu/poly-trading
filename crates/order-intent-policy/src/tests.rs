use super::*;
use accounting_ledger::LedgerRiskView;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use portfolio_risk::{
    BinaryMarketRisk, DecisionStatus, GroupMultiplier, OrderSide, PortfolioRiskEngine, RiskCommand,
    RiskCommandId, RiskLimits, RiskRequest, ShockProfile,
};
use proptest::prelude::*;
use settlement_reconciliation::ReconciliationRiskGate;
use std::fs;
use tempfile::tempdir;

fn cid(value: u8) -> PolicyCommandId {
    PolicyCommandId([value; 32])
}

fn oid(value: u8) -> RiskOrderId {
    RiskOrderId([value; 32])
}

fn up() -> TokenKey {
    TokenKey::new("btc", "btc-up").expect("token")
}

fn down() -> TokenKey {
    TokenKey::new("btc", "btc-down").expect("token")
}

fn order() -> OrderExposure {
    OrderExposure {
        order_id: oid(9),
        token: up(),
        side: OrderSide::Buy,
        quantity_micros: 100_000,
        partial_fill_micros: 50_000,
        limit_price_micros: 400_000,
        max_fee_micros: 1_000,
    }
}

fn approval(order: OrderExposure, approve: bool) -> RiskDecision {
    let ledger = LedgerRiskView {
        ledger_digest: [2; 32],
        halted: false,
        cash_available_micros: 1_000_000,
        cash_reserved_micros: 0,
        available_tokens: Vec::new(),
        reserved_tokens: Vec::new(),
        locked_tokens: Vec::new(),
    };
    let request = RiskRequest {
        reconciliation: ReconciliationRiskGate {
            reconciliation_digest: [1; 32],
            ready: true,
            evaluated_at_ns: Some(100),
            ledger_digest: Some([2; 32]),
            chain_block_number: Some(7),
        },
        ledger,
        markets: vec![BinaryMarketRisk {
            condition_id: "btc".to_owned(),
            up: up(),
            down: down(),
            shock_group: "crypto".to_owned(),
        }],
        open_orders: Vec::new(),
        candidate: order,
        additional_candidates: Vec::new(),
        shocks: vec![ShockProfile {
            shock_id: "baseline".to_owned(),
            group_multipliers: Vec::<GroupMultiplier>::new(),
        }],
        limits: RiskLimits {
            capital_floor_micros: if approve { 950_000 } else { 999_999 },
            operational_reserve_micros: 0,
            pending_settlement_reserve_micros: 0,
            max_gross_exposure_micros: 1_000_000,
            max_condition_exposure_micros: 1_000_000,
            max_group_exposure_micros: 1_000_000,
            reserved_cash_haircut_bps: 10_000,
            available_token_haircut_bps: 10_000,
            reserved_token_haircut_bps: 10_000,
            locked_token_haircut_bps: 10_000,
            max_reconciliation_age_ns: 100,
            max_open_orders: 4,
            max_scenarios: 100,
        },
        evaluated_at_ns: 110,
    };
    let mut engine = PortfolioRiskEngine::default();
    engine
        .apply(&RiskCommand::Evaluate {
            command_id: RiskCommandId([3; 32]),
            request,
            recorded_at_ns: 110,
        })
        .expect("risk decision")
}

fn signer() -> SignerPolicyFrame {
    SignerPolicyFrame {
        policy_id: [4; 32],
        venue: "polymarket".to_owned(),
        exchange_contract: "ctf-exchange-v1".to_owned(),
        allowed_tokens: vec![up()],
        max_quantity_micros: 200_000,
        max_price_micros: 500_000,
        max_notional_micros: 100_000,
        allow_maker: true,
        allow_taker: true,
        valid_from_ns: 100,
        valid_until_ns: 1_000,
    }
}

fn placement() -> PlacementRequest {
    let order = order();
    PlacementRequest {
        approval: approval(order.clone(), true),
        order,
        venue: "polymarket".to_owned(),
        exchange_contract: "ctf-exchange-v1".to_owned(),
        post_only: true,
        marketable: false,
        time_in_force: TimeInForce::Gtc,
        signer_policy: signer(),
        max_approval_age_ns: 100,
        max_mode_age_ns: 100,
        authorization_expires_at_ns: 200,
        evaluated_at_ns: 120,
    }
}

fn observe(id: u8, sequence: u64, mode: ExchangeMode, at: i64) -> PolicyCommand {
    PolicyCommand::ObserveMode {
        command_id: cid(id),
        observation: ExchangeModeObservation {
            sequence,
            mode,
            observed_at_ns: at,
            valid_until_ns: at + 500,
        },
        recorded_at_ns: at,
    }
}

fn place(id: u8, request: PlacementRequest) -> PolicyCommand {
    let at = request.evaluated_at_ns;
    PolicyCommand::AuthorizePlacement {
        command_id: cid(id),
        request: Box::new(request),
        recorded_at_ns: at,
    }
}

fn ready_engine(mode: ExchangeMode) -> IntentPolicyEngine {
    let mut engine = IntentPolicyEngine::default();
    engine.apply(&observe(1, 0, mode, 100)).expect("mode");
    engine
}

#[test]
fn normal_mode_binds_exact_risk_order_and_signer_policy() {
    let mut engine = ready_engine(ExchangeMode::Normal);
    let request = placement();
    let expected_risk = request.approval.decision_digest;
    let expected_signer = request.signer_policy.digest();
    let decision = engine.apply(&place(2, request)).expect("permit");
    assert_eq!(decision.status, PolicyStatus::Permit);
    assert_eq!(decision.reason, PolicyReason::PlacementPermitted);
    assert_eq!(decision.risk_decision_digest, Some(expected_risk));
    assert_eq!(decision.signer_policy_digest, Some(expected_signer));
    assert!(decision.verify_digest());
    assert_eq!(engine.snapshot().used_approvals, 1);
}

#[test]
fn every_exchange_mode_has_an_explicit_placement_rule() {
    for mode in [
        ExchangeMode::Restarting,
        ExchangeMode::CancelOnly,
        ExchangeMode::TradingDisabled,
        ExchangeMode::Recovering,
        ExchangeMode::Unknown,
    ] {
        let mut engine = ready_engine(mode);
        let decision = engine.apply(&place(2, placement())).expect("decision");
        assert_eq!(decision.status, PolicyStatus::Deny);
        assert_eq!(decision.reason, PolicyReason::ModeForbidsPlacement);
    }
    let mut absent = IntentPolicyEngine::default();
    assert_eq!(
        absent
            .apply(&place(2, placement()))
            .expect("decision")
            .reason,
        PolicyReason::ModeUnavailable
    );
}

#[test]
fn post_only_mode_rejects_marketable_or_non_post_only_orders() {
    let mut engine = ready_engine(ExchangeMode::PostOnly);
    assert_eq!(
        engine.apply(&place(2, placement())).expect("permit").status,
        PolicyStatus::Permit
    );

    for (post_only, marketable) in [(false, false), (true, true)] {
        let mut engine = ready_engine(ExchangeMode::PostOnly);
        let mut request = placement();
        request.post_only = post_only;
        request.marketable = marketable;
        assert_eq!(
            engine.apply(&place(2, request)).expect("deny").reason,
            PolicyReason::PostOnlyViolation
        );
    }
}

#[test]
fn risk_decision_authenticity_order_binding_age_and_status_are_required() {
    let mut tampered = placement();
    tampered.approval.scenario_count += 1;
    assert_eq!(
        ready_engine(ExchangeMode::Normal)
            .apply(&place(2, tampered))
            .expect("deny")
            .reason,
        PolicyReason::RiskDigestInvalid
    );

    let mut mismatch = placement();
    mismatch.order.limit_price_micros += 1;
    assert_eq!(
        ready_engine(ExchangeMode::Normal)
            .apply(&place(2, mismatch))
            .expect("deny")
            .reason,
        PolicyReason::RiskOrderMismatch
    );

    let mut stale = placement();
    stale.max_approval_age_ns = 9;
    assert_eq!(
        ready_engine(ExchangeMode::Normal)
            .apply(&place(2, stale))
            .expect("deny")
            .reason,
        PolicyReason::RiskApprovalStale
    );

    let mut rejected = placement();
    rejected.approval = approval(rejected.order.clone(), false);
    assert_eq!(rejected.approval.status, DecisionStatus::NoTrade);
    assert_eq!(
        ready_engine(ExchangeMode::Normal)
            .apply(&place(2, rejected))
            .expect("deny")
            .reason,
        PolicyReason::RiskNotApproved
    );
}

#[test]
fn signer_policy_enforces_all_static_boundaries() {
    let cases = [
        (0, PolicyReason::QuantityLimit),
        (1, PolicyReason::PriceLimit),
        (2, PolicyReason::NotionalLimit),
        (3, PolicyReason::VenueForbidden),
        (4, PolicyReason::ContractForbidden),
        (5, PolicyReason::TokenForbidden),
        (6, PolicyReason::MakerForbidden),
        (7, PolicyReason::SignerPolicyInactive),
    ];
    for (case, expected) in cases {
        let mut request = placement();
        match case {
            0 => request.signer_policy.max_quantity_micros = 99_999,
            1 => request.signer_policy.max_price_micros = 399_999,
            2 => request.signer_policy.max_notional_micros = 40_999,
            3 => request.venue = "other".to_owned(),
            4 => request.exchange_contract = "other".to_owned(),
            5 => request.signer_policy.allowed_tokens = vec![down()],
            6 => request.signer_policy.allow_maker = false,
            7 => request.signer_policy.valid_until_ns = 120,
            _ => unreachable!(),
        }
        assert_eq!(
            ready_engine(ExchangeMode::Normal)
                .apply(&place(2, request))
                .expect("deny")
                .reason,
            expected
        );
    }
}

#[test]
fn taker_and_time_in_force_require_explicit_policy() {
    let mut taker = placement();
    taker.post_only = false;
    taker.marketable = true;
    taker.time_in_force = TimeInForce::Fok;
    taker.signer_policy.allow_taker = false;
    assert_eq!(
        ready_engine(ExchangeMode::Normal)
            .apply(&place(2, taker))
            .expect("deny")
            .reason,
        PolicyReason::TakerForbidden
    );

    let mut expired = placement();
    expired.time_in_force = TimeInForce::Gtd { expires_at_ns: 120 };
    assert_eq!(
        ready_engine(ExchangeMode::Normal)
            .apply(&place(2, expired))
            .expect("deny")
            .reason,
        PolicyReason::TimeInForceInvalid
    );
}

#[test]
fn mode_and_authorization_expiry_boundaries_fail_closed() {
    let mut exact_mode_age = placement();
    exact_mode_age.max_mode_age_ns = 20;
    assert_eq!(
        ready_engine(ExchangeMode::Normal)
            .apply(&place(2, exact_mode_age))
            .expect("permit")
            .status,
        PolicyStatus::Permit
    );

    let mut stale_mode = placement();
    stale_mode.max_mode_age_ns = 19;
    assert_eq!(
        ready_engine(ExchangeMode::Normal)
            .apply(&place(2, stale_mode))
            .expect("deny")
            .reason,
        PolicyReason::ModeStale
    );

    let mut expired_observation = IntentPolicyEngine::default();
    expired_observation
        .apply(&PolicyCommand::ObserveMode {
            command_id: cid(1),
            observation: ExchangeModeObservation {
                sequence: 0,
                mode: ExchangeMode::Normal,
                observed_at_ns: 100,
                valid_until_ns: 120,
            },
            recorded_at_ns: 100,
        })
        .expect("mode");
    assert_eq!(
        expired_observation
            .apply(&place(2, placement()))
            .expect("deny")
            .reason,
        PolicyReason::ModeStale
    );

    let mut expired_authorization = placement();
    expired_authorization.authorization_expires_at_ns = 120;
    assert_eq!(
        ready_engine(ExchangeMode::Normal)
            .apply(&place(2, expired_authorization))
            .expect("deny")
            .reason,
        PolicyReason::AuthorizationExpired
    );
}

#[test]
fn approval_and_order_cannot_be_authorized_twice() {
    let mut engine = ready_engine(ExchangeMode::Normal);
    engine.apply(&place(2, placement())).expect("permit");
    assert_eq!(
        engine.apply(&place(3, placement())).expect("deny").reason,
        PolicyReason::ApprovalAlreadyUsed
    );
}

#[test]
fn delayed_orders_are_uncancellable_until_exact_boundary() {
    let mut engine = ready_engine(ExchangeMode::Normal);
    engine.apply(&place(2, placement())).expect("place");
    engine
        .apply(&PolicyCommand::MarkDelayed {
            command_id: cid(3),
            order_id: oid(9),
            release_at_ns: 150,
            uncancellable_until_ns: 160,
            recorded_at_ns: 130,
        })
        .expect("delay");
    let cancel = |id, at| PolicyCommand::AuthorizeCancel {
        command_id: cid(id),
        request: CancelRequest {
            order_id: oid(9),
            max_mode_age_ns: 100,
            evaluated_at_ns: at,
        },
        recorded_at_ns: at,
    };
    assert_eq!(
        engine.apply(&cancel(4, 159)).expect("deny").reason,
        PolicyReason::Uncancellable
    );
    assert_eq!(
        engine.apply(&cancel(5, 160)).expect("permit").status,
        PolicyStatus::Permit
    );
}

#[test]
fn cancel_mode_matrix_reduces_exposure_but_restart_fails_closed() {
    for mode in [
        ExchangeMode::Normal,
        ExchangeMode::PostOnly,
        ExchangeMode::CancelOnly,
        ExchangeMode::TradingDisabled,
        ExchangeMode::Recovering,
    ] {
        let mut engine = ready_engine(ExchangeMode::Normal);
        engine.apply(&place(2, placement())).expect("place");
        engine.apply(&observe(3, 1, mode, 130)).expect("mode");
        let decision = engine
            .apply(&PolicyCommand::AuthorizeCancel {
                command_id: cid(4),
                request: CancelRequest {
                    order_id: oid(9),
                    max_mode_age_ns: 100,
                    evaluated_at_ns: 140,
                },
                recorded_at_ns: 140,
            })
            .expect("cancel");
        assert_eq!(decision.status, PolicyStatus::Permit);
    }
    let mut restarting = ready_engine(ExchangeMode::Normal);
    restarting.apply(&place(2, placement())).expect("place");
    restarting
        .apply(&observe(3, 1, ExchangeMode::Restarting, 130))
        .expect("mode");
    assert_eq!(
        restarting
            .apply(&PolicyCommand::AuthorizeCancel {
                command_id: cid(4),
                request: CancelRequest {
                    order_id: oid(9),
                    max_mode_age_ns: 100,
                    evaluated_at_ns: 140,
                },
                recorded_at_ns: 140,
            })
            .expect("deny")
            .reason,
        PolicyReason::ModeForbidsCancel
    );
}

#[test]
fn premature_release_terminal_mutation_and_mode_equivocation_halt() {
    let mut exact_release = ready_engine(ExchangeMode::Normal);
    exact_release.apply(&place(2, placement())).expect("place");
    exact_release
        .apply(&PolicyCommand::MarkDelayed {
            command_id: cid(3),
            order_id: oid(9),
            release_at_ns: 150,
            uncancellable_until_ns: 160,
            recorded_at_ns: 130,
        })
        .expect("delay");
    assert_eq!(
        exact_release
            .apply(&PolicyCommand::MarkLive {
                command_id: cid(4),
                order_id: oid(9),
                recorded_at_ns: 150,
            })
            .expect("live")
            .status,
        PolicyStatus::Permit
    );

    let mut lifecycle = ready_engine(ExchangeMode::Normal);
    lifecycle.apply(&place(2, placement())).expect("place");
    lifecycle
        .apply(&PolicyCommand::MarkDelayed {
            command_id: cid(3),
            order_id: oid(9),
            release_at_ns: 150,
            uncancellable_until_ns: 150,
            recorded_at_ns: 130,
        })
        .expect("delay");
    assert_eq!(
        lifecycle.apply(&PolicyCommand::MarkLive {
            command_id: cid(4),
            order_id: oid(9),
            recorded_at_ns: 149,
        }),
        Err(Error::LifecycleTransition)
    );
    assert!(lifecycle.is_halted());

    let mut terminal = ready_engine(ExchangeMode::Normal);
    terminal.apply(&place(2, placement())).expect("place");
    terminal
        .apply(&PolicyCommand::MarkTerminal {
            command_id: cid(3),
            order_id: oid(9),
            recorded_at_ns: 130,
        })
        .expect("terminal");
    assert_eq!(
        terminal.apply(&PolicyCommand::MarkTerminal {
            command_id: cid(4),
            order_id: oid(9),
            recorded_at_ns: 131,
        }),
        Err(Error::LifecycleTransition)
    );

    let mut history = ready_engine(ExchangeMode::Normal);
    assert_eq!(
        history.apply(&observe(2, 0, ExchangeMode::PostOnly, 100)),
        Err(Error::ModeHistory)
    );
}

#[test]
fn command_idempotency_codec_and_clock_history_are_strict() {
    let command = observe(1, 0, ExchangeMode::Normal, 100);
    let encoded = encode_command(&command).expect("encode");
    assert_eq!(decode_command(&encoded), Ok(command.clone()));
    let mut trailing = encoded;
    trailing.extend_from_slice(b" {}");
    assert!(matches!(decode_command(&trailing), Err(Error::Json(_))));

    let mut engine = IntentPolicyEngine::default();
    let first = engine.apply(&command).expect("first");
    assert_eq!(engine.apply(&command), Ok(first));
    assert_eq!(
        engine.apply(&observe(1, 0, ExchangeMode::PostOnly, 100)),
        Err(Error::IdempotencyConflict)
    );

    let mut clock = ready_engine(ExchangeMode::Normal);
    assert_eq!(
        clock.apply(&observe(2, 1, ExchangeMode::Normal, 99)),
        Err(Error::ClockRegression)
    );
}

#[test]
fn segmented_replay_checkpoint_corruption_and_sync_failure_are_fail_closed() {
    let directory = tempdir().expect("directory");
    let writer = SegmentedJournalWriter::open(
        directory.path(),
        SegmentConfig {
            max_segment_bytes: 1_024 * 1_024,
            max_segment_records: 1,
        },
    )
    .expect("writer");
    let mut durable = DurablePolicyEngine::new(
        writer,
        PolicyRecovery {
            engine: IntentPolicyEngine::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    durable
        .apply(&observe(1, 0, ExchangeMode::Normal, 100))
        .expect("mode");
    let checkpoint = PolicyCheckpoint {
        sequence: 0,
        policy_digest: durable.engine().snapshot().digest,
    };
    durable.apply(&place(2, placement())).expect("place");
    let online = durable.engine().snapshot().digest;
    drop(durable);
    let recovered = recover_segmented(directory.path(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.engine.snapshot().digest, online);

    let path = directory.path().join("policy.checkpoint");
    write_checkpoint_create_new(&path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    let corrupt = directory.path().join("corrupt.checkpoint");
    let mut bytes = fs::read(&path).expect("bytes");
    bytes[24] ^= 1;
    fs::write(&corrupt, bytes).expect("corrupt");
    assert!(matches!(
        read_checkpoint(corrupt),
        Err(StorageError::CheckpointChecksum)
    ));

    let mut failed = DurablePolicyEngine::new(
        FailingJournal::default(),
        PolicyRecovery {
            engine: IntentPolicyEngine::default(),
            last_sequence: None,
        },
    )
    .expect("failing");
    assert!(matches!(
        failed.apply(&observe(1, 0, ExchangeMode::Normal, 100)),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(failed.engine().snapshot().accepted_commands, 0);
}

#[derive(Debug, Default)]
struct FailingJournal {
    last: Option<u64>,
}

impl EventJournal for FailingJournal {
    fn append_event(
        &mut self,
        event: &event_schema::EventEnvelope,
    ) -> Result<u64, JournalBackendError> {
        self.last = Some(event.sequence);
        Ok(0)
    }

    fn sync_events(&self) -> Result<(), JournalBackendError> {
        Err(JournalBackendError::Single(
            market_recorder::JournalError::Io(std::io::Error::other("injected sync")),
        ))
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}

proptest! {
    #[test]
    fn stricter_quantity_or_notional_never_turns_denial_into_permit(
        quantity_limit in 1_i128..100_000,
        notional_limit in 1_i128..41_000,
    ) {
        let mut request = placement();
        request.signer_policy.max_quantity_micros = quantity_limit;
        request.signer_policy.max_notional_micros = notional_limit;
        let decision = ready_engine(ExchangeMode::Normal)
            .apply(&place(2, request))
            .expect("decision");
        prop_assert_eq!(decision.status, PolicyStatus::Deny);
    }
}
