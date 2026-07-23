use super::*;
use accounting_ledger::LedgerRiskView;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use order_intent_policy::{
    ExchangeMode, ExchangeModeObservation, IntentPolicyEngine, PolicyCommand, PolicyCommandId,
    SignerPolicyFrame, TimeInForce,
};
use portfolio_risk::{
    BinaryMarketRisk, GroupMultiplier, OrderExposure, PortfolioRiskEngine, RiskCommand,
    RiskCommandId, RiskLimits, RiskRequest, ShockProfile,
};
use proptest::prelude::*;
use settlement_reconciliation::ReconciliationRiskGate;
use std::fs;
use tempfile::tempdir;

fn eid(value: u8) -> ExecutionCommandId {
    ExecutionCommandId([value; 32])
}

fn pid(value: u8) -> PolicyCommandId {
    PolicyCommandId([value; 32])
}

fn oid() -> RiskOrderId {
    RiskOrderId([9; 32])
}

fn token(name: &str) -> accounting_ledger::TokenKey {
    accounting_ledger::TokenKey::new("btc", name).expect("token")
}

fn order() -> OrderExposure {
    OrderExposure {
        order_id: oid(),
        token: token("up"),
        side: OrderSide::Buy,
        quantity_micros: 100_000,
        partial_fill_micros: 50_000,
        limit_price_micros: 400_000,
        max_fee_micros: 1_000,
    }
}

fn risk_approval(order: OrderExposure) -> portfolio_risk::RiskDecision {
    let request = RiskRequest {
        reconciliation: ReconciliationRiskGate {
            reconciliation_digest: [1; 32],
            ready: true,
            evaluated_at_ns: Some(100),
            ledger_digest: Some([2; 32]),
            chain_block_number: Some(7),
        },
        ledger: LedgerRiskView {
            ledger_digest: [2; 32],
            halted: false,
            cash_available_micros: 1_000_000,
            cash_reserved_micros: 0,
            available_tokens: Vec::new(),
            reserved_tokens: Vec::new(),
            locked_tokens: Vec::new(),
        },
        markets: vec![BinaryMarketRisk {
            condition_id: "btc".to_owned(),
            up: token("up"),
            down: token("down"),
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
            capital_floor_micros: 950_000,
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
    PortfolioRiskEngine::default()
        .apply(&RiskCommand::Evaluate {
            command_id: RiskCommandId([3; 32]),
            request,
            recorded_at_ns: 110,
        })
        .expect("risk")
}

fn placement() -> PlacementRequest {
    let order = order();
    PlacementRequest {
        approval: risk_approval(order.clone()),
        order,
        venue: "polymarket".to_owned(),
        exchange_contract: "ctf".to_owned(),
        post_only: true,
        marketable: false,
        time_in_force: TimeInForce::Gtc,
        signer_policy: SignerPolicyFrame {
            policy_id: [4; 32],
            venue: "polymarket".to_owned(),
            exchange_contract: "ctf".to_owned(),
            allowed_tokens: vec![token("up")],
            max_quantity_micros: 100_000,
            max_price_micros: 400_000,
            max_notional_micros: 41_000,
            allow_maker: true,
            allow_taker: false,
            valid_from_ns: 100,
            valid_until_ns: 1_000,
        },
        max_approval_age_ns: 100,
        max_mode_age_ns: 100,
        authorization_expires_at_ns: 500,
        evaluated_at_ns: 120,
    }
}

fn policy_fixture() -> (
    PlacementRequest,
    PolicyDecision,
    CancelRequest,
    PolicyDecision,
) {
    let placement = placement();
    let mut engine = IntentPolicyEngine::default();
    engine
        .apply(&PolicyCommand::ObserveMode {
            command_id: pid(1),
            observation: ExchangeModeObservation {
                sequence: 0,
                mode: ExchangeMode::Normal,
                observed_at_ns: 100,
                valid_until_ns: 1_000,
            },
            recorded_at_ns: 100,
        })
        .expect("mode");
    let place = engine
        .apply(&PolicyCommand::AuthorizePlacement {
            command_id: pid(2),
            request: Box::new(placement.clone()),
            recorded_at_ns: 120,
        })
        .expect("place");
    let cancel = CancelRequest {
        order_id: oid(),
        max_mode_age_ns: 100,
        evaluated_at_ns: 140,
    };
    let cancel_decision = engine
        .apply(&PolicyCommand::AuthorizeCancel {
            command_id: pid(3),
            request: cancel.clone(),
            recorded_at_ns: 140,
        })
        .expect("cancel");
    (placement, place, cancel, cancel_decision)
}

fn submit(
    id: u8,
    placement: PlacementRequest,
    decision: PolicyDecision,
    at: i64,
) -> ExecutionCommand {
    ExecutionCommand::Submit {
        command_id: eid(id),
        policy_decision: decision,
        placement: Box::new(placement),
        local_submission_id: "local-1".to_owned(),
        recorded_at_ns: at,
    }
}

fn observe(id: u8, sequence: u64, at: i64, event: ExchangeEvent) -> ExecutionCommand {
    ExecutionCommand::Observe {
        command_id: eid(id),
        observation: Box::new(ExchangeObservation {
            order_id: oid(),
            source_sequence: sequence,
            exchange_order_id: Some("exchange-1".to_owned()),
            event,
            event_time_ns: at,
            received_time_ns: at,
        }),
        recorded_at_ns: at,
    }
}

fn fill(id: &str, quantity: i128, cumulative: i128, fully: bool) -> ExchangeEvent {
    ExchangeEvent::Match {
        fill: MatchFill {
            fill_id: id.to_owned(),
            quantity_micros: quantity,
            consideration_micros: quantity * 4 / 10,
            fee_micros: 0,
            cumulative_quantity_micros: cumulative,
            cumulative_consideration_micros: cumulative * 4 / 10,
            cumulative_fee_micros: 0,
            ledger_command_id: LedgerCommandId([id.as_bytes()[0]; 32]),
        },
        fully_matched: fully,
    }
}

fn submitted() -> (PaperExecutionEngine, CancelRequest, PolicyDecision) {
    let (placement, permit, cancel, cancel_permit) = policy_fixture();
    let mut engine = PaperExecutionEngine::default();
    engine
        .apply(&submit(1, placement, permit, 130))
        .expect("submit");
    (engine, cancel, cancel_permit)
}

#[test]
fn submission_requires_exact_authentic_unexpired_policy_subject() {
    let (placement, permit, _, _) = policy_fixture();
    let mut engine = PaperExecutionEngine::default();
    let decision = engine
        .apply(&submit(1, placement.clone(), permit.clone(), 130))
        .expect("submit");
    assert_eq!(decision.status, ExecutionStatus::Applied);
    assert!(decision.verify_digest());
    assert!(matches!(
        engine.order(oid()).expect("order").state,
        OrderState::Active(ActiveState::Submitted)
    ));

    let mut tampered = permit.clone();
    tampered.reason = PolicyReason::ModeAccepted;
    assert_eq!(
        PaperExecutionEngine::default()
            .apply(&submit(1, placement.clone(), tampered, 130))
            .expect("deny")
            .reason,
        ExecutionReason::PolicyDigestInvalid
    );

    let mut substituted = placement.clone();
    substituted.order.limit_price_micros -= 1;
    assert_eq!(
        PaperExecutionEngine::default()
            .apply(&submit(1, substituted, permit.clone(), 130))
            .expect("deny")
            .reason,
        ExecutionReason::PolicySubjectMismatch
    );
    assert_eq!(
        PaperExecutionEngine::default()
            .apply(&submit(1, placement.clone(), permit.clone(), 500))
            .expect("deny")
            .reason,
        ExecutionReason::AuthorizationExpired
    );
    assert_eq!(
        PaperExecutionEngine::default()
            .apply(&submit(1, placement, permit, 119))
            .expect("deny")
            .reason,
        ExecutionReason::AuthorizationNotYetValid
    );
}

#[test]
fn placement_permit_and_order_are_single_use() {
    let (placement, permit, _, _) = policy_fixture();
    let mut engine = PaperExecutionEngine::default();
    engine
        .apply(&submit(1, placement.clone(), permit.clone(), 130))
        .expect("submit");
    assert_eq!(
        engine
            .apply(&submit(2, placement, permit, 131))
            .expect("deny")
            .reason,
        ExecutionReason::PolicyAlreadyUsed
    );
}

#[test]
fn delayed_order_cannot_go_live_early_and_can_reject_after_delay() {
    let (mut early, _, _) = submitted();
    early
        .apply(&observe(
            2,
            0,
            150,
            ExchangeEvent::Delayed {
                release_at_ns: 170,
                uncancellable_until_ns: 170,
            },
        ))
        .expect("delay");
    assert_eq!(
        early.apply(&observe(3, 1, 169, ExchangeEvent::Live)),
        Err(Error::LifecycleTransition)
    );

    let (mut rejected, _, _) = submitted();
    rejected
        .apply(&observe(
            2,
            0,
            150,
            ExchangeEvent::Delayed {
                release_at_ns: 170,
                uncancellable_until_ns: 180,
            },
        ))
        .expect("delay");
    rejected
        .apply(&observe(
            3,
            1,
            170,
            ExchangeEvent::Rejected {
                class: RetryClass::DelayedCheck,
                code: "balance".to_owned(),
            },
        ))
        .expect("reject");
    assert!(matches!(
        rejected.order(oid()).expect("order").state,
        OrderState::Rejected {
            class: RetryClass::DelayedCheck,
            ..
        }
    ));
}

#[test]
fn acknowledgement_live_and_exchange_identity_are_immutable() {
    let (mut engine, _, _) = submitted();
    engine
        .apply(&observe(2, 0, 135, ExchangeEvent::Acknowledged))
        .expect("ack");
    engine
        .apply(&observe(3, 1, 151, ExchangeEvent::Live))
        .expect("live");
    let mut changed = observe(4, 2, 152, ExchangeEvent::Live);
    let ExecutionCommand::Observe { observation, .. } = &mut changed else {
        unreachable!()
    };
    observation.exchange_order_id = Some("other".to_owned());
    assert_eq!(engine.apply(&changed), Err(Error::ExchangeOrderIdentity));
}

#[test]
fn unknown_is_nonterminal_and_newer_observation_recovers() {
    let (mut engine, _, _) = submitted();
    engine
        .apply(&observe(
            2,
            0,
            150,
            ExchangeEvent::Unknown {
                reason: UnknownReason::SubmitTimeout,
            },
        ))
        .expect("unknown");
    assert!(!engine.order(oid()).expect("order").state.is_terminal());
    engine
        .apply(&observe(3, 1, 151, ExchangeEvent::Acknowledged))
        .expect("recover");
    assert!(matches!(
        engine.order(oid()).expect("order").state,
        OrderState::Active(ActiveState::Acknowledged)
    ));
}

#[test]
fn partial_and_full_matches_emit_exact_reconciliation_handoffs() {
    let (mut engine, _, _) = submitted();
    engine
        .apply(&observe(2, 0, 150, ExchangeEvent::Acknowledged))
        .expect("ack");
    let first = engine
        .apply(&observe(3, 1, 151, fill("a", 50_000, 50_000, false)))
        .expect("partial");
    let handoff = first.new_handoff.expect("handoff");
    assert_eq!(handoff.intent.quantity_micros, 50_000);
    assert_eq!(handoff.intent.consideration_micros, 20_000);
    assert_eq!(handoff.intent.order_id, "exchange-1");
    let second = engine
        .apply(&observe(4, 2, 152, fill("b", 50_000, 100_000, true)))
        .expect("full");
    assert!(second.new_handoff.is_some());
    let order = engine.order(oid()).expect("order");
    assert_eq!(order.handoffs.len(), 2);
    assert_eq!(
        order
            .handoffs
            .iter()
            .map(|value| value.intent.quantity_micros)
            .sum::<i128>(),
        100_000
    );
    assert_eq!(order.state, OrderState::FullyMatched);
}

#[test]
fn invalid_cumulative_limit_fee_and_duplicate_fill_halt() {
    for case in 0..5 {
        let (mut engine, _, _) = submitted();
        engine
            .apply(&observe(2, 0, 150, ExchangeEvent::Acknowledged))
            .expect("ack");
        let mut event = fill("a", 50_000, 50_000, false);
        let ExchangeEvent::Match {
            fill: fill_data, ..
        } = &mut event
        else {
            unreachable!()
        };
        match case {
            0 => fill_data.cumulative_quantity_micros += 1,
            1 => fill_data.consideration_micros = 20_001,
            2 => fill_data.fee_micros = 1_001,
            3 | 4 => {}
            _ => unreachable!(),
        }
        if case < 3 {
            assert!(matches!(
                engine.apply(&observe(3, 1, 151, event)),
                Err(Error::FillInvariant)
            ));
        } else {
            engine.apply(&observe(3, 1, 151, event)).expect("first");
            let mut repeated = fill(if case == 3 { "a" } else { "b" }, 50_000, 100_000, true);
            if case == 4 {
                let ExchangeEvent::Match { fill, .. } = &mut repeated else {
                    unreachable!()
                };
                fill.ledger_command_id = LedgerCommandId([b'a'; 32]);
            }
            assert!(matches!(
                engine.apply(&observe(4, 2, 152, repeated)),
                Err(Error::FillInvariant)
            ));
        }
    }
}

#[test]
fn cancel_pending_can_partially_match_then_cancel() {
    let (mut engine, cancel, permit) = submitted();
    engine
        .apply(&observe(2, 0, 135, ExchangeEvent::Acknowledged))
        .expect("ack");
    engine
        .apply(&ExecutionCommand::RequestCancel {
            command_id: eid(3),
            policy_decision: permit,
            request: cancel,
            recorded_at_ns: 140,
        })
        .expect("cancel request");
    engine
        .apply(&observe(4, 1, 151, fill("a", 50_000, 50_000, false)))
        .expect("race fill");
    assert!(matches!(
        engine.order(oid()).expect("order").state,
        OrderState::CancelPending {
            resume: ActiveState::PartiallyMatched
        }
    ));
    engine
        .apply(&observe(5, 2, 152, ExchangeEvent::CancelAccepted))
        .expect("canceled");
    assert_eq!(
        engine.order(oid()).expect("order").state,
        OrderState::Canceled
    );
}

#[test]
fn full_match_wins_cancel_race_and_cancel_rejection_restores_state() {
    let (mut full, cancel, permit) = submitted();
    full.apply(&observe(2, 0, 135, ExchangeEvent::Acknowledged))
        .expect("ack");
    full.apply(&ExecutionCommand::RequestCancel {
        command_id: eid(3),
        policy_decision: permit,
        request: cancel,
        recorded_at_ns: 140,
    })
    .expect("cancel");
    full.apply(&observe(4, 1, 151, fill("a", 100_000, 100_000, true)))
        .expect("full");
    assert_eq!(
        full.order(oid()).expect("order").state,
        OrderState::FullyMatched
    );

    let (mut rejected, cancel, permit) = submitted();
    rejected
        .apply(&observe(2, 0, 135, ExchangeEvent::Live))
        .expect("live");
    rejected
        .apply(&ExecutionCommand::RequestCancel {
            command_id: eid(3),
            policy_decision: permit,
            request: cancel,
            recorded_at_ns: 140,
        })
        .expect("cancel");
    rejected
        .apply(&observe(4, 1, 151, ExchangeEvent::CancelRejected))
        .expect("rejected");
    assert!(matches!(
        rejected.order(oid()).expect("order").state,
        OrderState::Active(ActiveState::Live)
    ));
}

#[test]
fn cancel_policy_binding_and_replay_are_enforced() {
    let (mut denied, cancel, permit) = submitted();
    let mut substitute = cancel.clone();
    substitute.evaluated_at_ns += 1;
    assert_eq!(
        denied
            .apply(&ExecutionCommand::RequestCancel {
                command_id: eid(2),
                policy_decision: permit.clone(),
                request: substitute,
                recorded_at_ns: 141
            })
            .expect("deny")
            .reason,
        ExecutionReason::PolicySubjectMismatch
    );
    let (mut engine, cancel, permit) = submitted();
    engine
        .apply(&ExecutionCommand::RequestCancel {
            command_id: eid(3),
            policy_decision: permit.clone(),
            request: cancel.clone(),
            recorded_at_ns: 140,
        })
        .expect("cancel");
    assert_eq!(
        engine
            .apply(&ExecutionCommand::RequestCancel {
                command_id: eid(4),
                policy_decision: permit,
                request: cancel,
                recorded_at_ns: 140
            })
            .expect("deny")
            .reason,
        ExecutionReason::PolicyAlreadyUsed
    );
}

#[test]
fn source_regression_retry_classes_idempotency_and_codec_are_strict() {
    let (placement, permit, _, _) = policy_fixture();
    let command = submit(1, placement, permit, 130);
    let encoded = encode_command(&command).expect("encode");
    assert_eq!(decode_command(&encoded), Ok(command.clone()));
    let mut engine = PaperExecutionEngine::default();
    let first = engine.apply(&command).expect("first");
    assert_eq!(engine.apply(&command), Ok(first));
    engine
        .apply(&observe(2, 10, 150, ExchangeEvent::Acknowledged))
        .expect("ack");
    assert_eq!(
        engine.apply(&observe(3, 10, 151, ExchangeEvent::Live)),
        Err(Error::SourceHistory)
    );

    let (mut receive_history, _, _) = submitted();
    let mut late_ack = observe(2, 10, 150, ExchangeEvent::Acknowledged);
    let ExecutionCommand::Observe {
        observation,
        recorded_at_ns,
        ..
    } = &mut late_ack
    else {
        unreachable!()
    };
    observation.received_time_ns = 200;
    *recorded_at_ns = 200;
    receive_history.apply(&late_ack).expect("ack");
    let mut receive_regression = observe(3, 11, 151, ExchangeEvent::Live);
    let ExecutionCommand::Observe {
        observation,
        recorded_at_ns,
        ..
    } = &mut receive_regression
    else {
        unreachable!()
    };
    observation.received_time_ns = 199;
    *recorded_at_ns = 201;
    assert_eq!(
        receive_history.apply(&receive_regression),
        Err(Error::SourceHistory)
    );

    for class in [
        RetryClass::Permanent,
        RetryClass::Restart,
        RetryClass::RateLimit,
        RetryClass::BalanceOrAllowance,
        RetryClass::DelayedCheck,
        RetryClass::Unknown,
    ] {
        let (mut value, _, _) = submitted();
        value
            .apply(&observe(
                2,
                0,
                150,
                ExchangeEvent::Rejected {
                    class,
                    code: "x".to_owned(),
                },
            ))
            .expect("reject");
        assert!(
            matches!(value.order(oid()).expect("order").state, OrderState::Rejected { class: actual, .. } if actual == class)
        );
    }
}

#[test]
fn durable_replay_checkpoint_corruption_and_sync_failure_are_fail_closed() {
    let directory = tempdir().expect("directory");
    let writer = SegmentedJournalWriter::open(
        directory.path(),
        SegmentConfig {
            max_segment_bytes: 1_024 * 1_024,
            max_segment_records: 1,
        },
    )
    .expect("writer");
    let mut durable = DurableExecutionEngine::new(
        writer,
        ExecutionRecovery {
            engine: PaperExecutionEngine::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    let (placement, permit, _, _) = policy_fixture();
    durable
        .apply(&submit(1, placement, permit, 130))
        .expect("submit");
    let checkpoint = ExecutionCheckpoint {
        sequence: 0,
        execution_digest: durable.engine().snapshot().digest,
    };
    durable
        .apply(&observe(2, 0, 150, ExchangeEvent::Acknowledged))
        .expect("ack");
    let online = durable.engine().snapshot().digest;
    drop(durable);
    assert_eq!(
        recover_segmented(directory.path(), Some(checkpoint))
            .expect("recover")
            .engine
            .snapshot()
            .digest,
        online
    );
    let path = directory.path().join("execution.checkpoint");
    write_checkpoint_create_new(&path, checkpoint).expect("write");
    let mut bytes = fs::read(&path).expect("bytes");
    bytes[24] ^= 1;
    let corrupt = directory.path().join("corrupt.checkpoint");
    fs::write(&corrupt, bytes).expect("corrupt");
    assert!(matches!(
        read_checkpoint(corrupt),
        Err(StorageError::CheckpointChecksum)
    ));
    let mut failed = DurableExecutionEngine::new(
        FailingJournal::default(),
        ExecutionRecovery {
            engine: PaperExecutionEngine::default(),
            last_sequence: None,
        },
    )
    .expect("failed");
    let (placement, permit, _, _) = policy_fixture();
    assert!(matches!(
        failed.apply(&submit(1, placement, permit, 130)),
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
            market_recorder::JournalError::Io(std::io::Error::other("sync")),
        ))
    }
    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}

proptest! {
    #[test]
    fn accepted_handoffs_sum_exactly_and_never_exceed_order(first_units in 1_i128..10_000) {
        let first = first_units * 10;
        let second = 100_000 - first;
        let (mut engine, _, _) = submitted();
        engine.apply(&observe(2, 0, 150, ExchangeEvent::Acknowledged)).expect("ack");
        engine.apply(&observe(3, 1, 151, fill("a", first, first, false))).expect("first");
        engine.apply(&observe(4, 2, 152, fill("b", second, 100_000, true))).expect("second");
        let order = engine.order(oid()).expect("order");
        let total: i128 = order.handoffs.iter().map(|value| value.intent.quantity_micros).sum();
        prop_assert_eq!(total, order.cumulative_quantity_micros);
        prop_assert!(total <= order.placement.order.quantity_micros);
    }
}
