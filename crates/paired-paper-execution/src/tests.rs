use super::*;
use common_types::{PriceMicros, QuantityMicros, QuotePriceMicros, ReferenceQuantityE8};
use complete_set_arbitrage::{
    ArbitrageCommand, ArbitrageCommandId, ArbitrageConstraints, ArbitrageDirection,
    ArbitrageEvaluationId, ArbitrageRequest,
};
use feed_supervisor::{SupervisorMode, SupervisorSnapshot};
use live_market_state::{ActorMode, ActorSnapshot};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use market_session::{
    CoordinationFrame, MarketSessionCoordinator, SessionKey, SessionSourceState, TokenBookView,
};
use order_intent_policy::{ExchangeMode, ExchangeModeObservation};
use paired_opportunity_runtime::{PairRiskFrame, PairedCommand, PairedCommandId};
use portfolio_risk::{BinaryMarketRisk, GroupMultiplier, RiskLimits, RiskOrderId, ShockProfile};
use proptest::prelude::*;
use public_market_data::{Asset, MarketIdentity, BTC_HOURLY};
use reference_market_data::{
    CandleData, CandleInterval, InProgressCandle, ReferenceHealth, ReferenceSnapshot,
    ReferenceSymbol,
};
use settlement_reconciliation::ReconciliationRiskGate;
use std::collections::BTreeMap;
use tempfile::tempdir;

const HOUR_MS: i64 = 3_600_000;
const ACTIVE_NS: i64 = HOUR_MS * 1_000_000;
const fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn context() -> strategy_proposal::StrategyContext {
    let identity = MarketIdentity {
        asset: Asset::Bitcoin, event_id: "event-a".into(), market_id: "market-a".into(),
        condition_id: format!("0x{}", "a".repeat(64)), question_id: format!("0x{}", "b".repeat(64)),
        event_slug: "event-a".into(), market_slug: "market-a".into(), series_id: BTC_HOURLY.id.into(),
        series_slug: BTC_HOURLY.slug.into(), title: "Up or Down".into(), start_time_ms: HOUR_MS,
        end_time_ms: 2 * HOUR_MS, resolution_source: "https://www.binance.com/en/trade/BTC_USDT".into(),
        description: "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the BTC/USDT 1 hour candle that begins on the time and date specified in the title. Otherwise, this market will resolve to \"Down\". The close and open displayed for the relevant candle will be used once the data for that candle is finalized. This market is about the Binance pair, not according to other exchanges or trading pairs.".into(),
        up_token_id: "up-a".into(), down_token_id: "down-a".into(), rules_fingerprint: bytes(7),
    };
    let book = |bid, ask| TokenBookView {
        authoritative: true,
        best_bid: Some((
            PriceMicros::new(bid).expect("bid"),
            QuantityMicros::new(2_000_000).expect("qty"),
        )),
        best_ask: Some((
            PriceMicros::new(ask).expect("ask"),
            QuantityMicros::new(2_000_000).expect("qty"),
        )),
    };
    let market = ActorSnapshot {
        mode: ActorMode::Ready,
        ready: true,
        epoch: 1,
        last_sequence: Some(10),
        book_count: 2,
        digest: bytes(3),
        last_market_event_ns: Some(ACTIVE_NS),
        last_market_received_ns: Some(ACTIVE_NS),
        halt_reason: None,
    };
    let reference = ReferenceSnapshot {
        health: ReferenceHealth::Ready,
        epoch: 2,
        last_sequence: Some(20),
        digest: bytes(4),
        last_reference_received_ns: Some(ACTIVE_NS),
        symbols: BTreeMap::new(),
    };
    let supervision = SupervisorSnapshot {
        mode: SupervisorMode::Ready,
        ready: true,
        evaluated_at_ns: Some(ACTIVE_NS),
        market_epoch: 1,
        market_sequence: Some(10),
        market_digest: bytes(5),
        market_state_digest: bytes(3),
        reference_epoch: 2,
        reference_sequence: Some(20),
        reference_digest: bytes(6),
        reference_state_digest: bytes(4),
        halt_reason: None,
        digest: bytes(8),
    };
    let candle = CandleData {
        symbol: ReferenceSymbol::BtcUsdt,
        interval: CandleInterval::OneHourUtc,
        open_time_ms: HOUR_MS,
        close_time_ms: 2 * HOUR_MS - 1,
        first_trade_id: 1,
        last_trade_id: 2,
        open: QuotePriceMicros::new(100_000_000).expect("open"),
        high: QuotePriceMicros::new(110_000_000).expect("high"),
        low: QuotePriceMicros::new(90_000_000).expect("low"),
        close: QuotePriceMicros::new(101_000_000).expect("close"),
        base_volume: ReferenceQuantityE8::new(100_000_000).expect("volume"),
        quote_volume: ReferenceQuantityE8::new(10_000_000_000).expect("volume"),
        trade_count: 2,
    };
    let key = SessionKey::from(&identity);
    let frame = CoordinationFrame {
        now_ns: ACTIVE_NS,
        market,
        reference,
        supervision,
        sessions: [(
            key,
            SessionSourceState {
                up_book: Some(book(390_000, 400_000)),
                down_book: Some(book(490_000, 500_000)),
                in_progress: Some(InProgressCandle(candle)),
                finalized: None,
            },
        )]
        .into_iter()
        .collect(),
    };
    let mut coordinator = MarketSessionCoordinator::default();
    coordinator.register(identity.clone()).expect("register");
    let snapshot = coordinator.evaluate(&frame).expect("coordinate");
    strategy_proposal::capture_context(&snapshot, &frame, &identity, ACTIVE_NS, ACTIVE_NS + 1_000)
        .expect("context")
}

fn paired(runtime: &PairedPaperExecution) -> PairedCommand {
    let context = context();
    let ledger = runtime.policy().staging().ledger_risk_view();
    let arbitrage = ArbitrageCommand::Evaluate {
        command_id: ArbitrageCommandId(bytes(31)),
        request: ArbitrageRequest {
            evaluation_id: ArbitrageEvaluationId(bytes(32)),
            direction: ArbitrageDirection::BuyPair,
            context: Box::new(context.clone()),
            constraints: ArbitrageConstraints {
                min_quantity_micros: 100_000,
                max_quantity_micros: 1_000_000,
                partial_fill_micros: 50_000,
                up_max_fee_micros: 4_000,
                down_max_fee_micros: 6_000,
                conversion_max_cost_micros: 1_000,
                min_net_profit_micros: 1,
                min_roi_bps: 1,
            },
            evaluated_at_ns: ACTIVE_NS,
            expires_at_ns: ACTIVE_NS + 100,
        },
        recorded_at_ns: ACTIVE_NS,
    };
    PairedCommand::Evaluate {
        command_id: PairedCommandId(bytes(33)),
        arbitrage_command: Box::new(arbitrage),
        risk_frame: Box::new(PairRiskFrame {
            reconciliation: ReconciliationRiskGate {
                reconciliation_digest: bytes(40),
                ready: true,
                evaluated_at_ns: Some(ACTIVE_NS),
                ledger_digest: Some(ledger.ledger_digest),
                chain_block_number: Some(7),
            },
            ledger,
            markets: vec![BinaryMarketRisk {
                condition_id: context.condition_id,
                up: context.up_token,
                down: context.down_token,
                shock_group: "crypto".into(),
            }],
            open_orders: Vec::new(),
            shocks: vec![ShockProfile {
                shock_id: "baseline".into(),
                group_multipliers: Vec::<GroupMultiplier>::new(),
            }],
            limits: RiskLimits {
                capital_floor_micros: 1_000_000,
                operational_reserve_micros: 0,
                pending_settlement_reserve_micros: 0,
                max_gross_exposure_micros: 3_000_000,
                max_condition_exposure_micros: 3_000_000,
                max_group_exposure_micros: 3_000_000,
                reserved_cash_haircut_bps: 10_000,
                available_token_haircut_bps: 10_000,
                reserved_token_haircut_bps: 10_000,
                locked_token_haircut_bps: 10_000,
                max_reconciliation_age_ns: 1_000,
                max_open_orders: 4,
                max_scenarios: 1_000,
            },
            evaluated_at_ns: ACTIVE_NS,
        }),
        recorded_at_ns: ACTIVE_NS,
    }
}

fn policy(
    runtime: &mut PairedPaperExecution,
    id: u8,
    command: PairedPolicyCommand,
) -> PairedExecutionDecision {
    let at = command.recorded_at_ns();
    runtime
        .apply(&PairedExecutionCommand::Policy {
            command_id: PairedExecutionCommandId(bytes(id)),
            command: Box::new(command),
            recorded_at_ns: at,
        })
        .expect("policy")
}

fn setup() -> (PairedPaperExecution, PairStageId, PairPermit) {
    let mut runtime = PairedPaperExecution::default();
    policy(
        &mut runtime,
        1,
        PairedPolicyCommand::Fund {
            command_id: PairedPolicyCommandId(bytes(1)),
            amount_micros: 3_000_000,
            recorded_at_ns: ACTIVE_NS - 2,
        },
    );
    let pair = paired(&runtime);
    let staged = policy(
        &mut runtime,
        2,
        PairedPolicyCommand::Stage {
            command_id: PairedPolicyCommandId(bytes(2)),
            paired_command: Box::new(pair),
            recorded_at_ns: ACTIVE_NS,
        },
    );
    let stage_id = staged
        .policy_decision
        .expect("staged")
        .stage_id
        .expect("stage");
    policy(
        &mut runtime,
        3,
        PairedPolicyCommand::ObserveMode {
            command_id: PairedPolicyCommandId(bytes(3)),
            observation: ExchangeModeObservation {
                sequence: 1,
                mode: ExchangeMode::Normal,
                observed_at_ns: ACTIVE_NS,
                valid_until_ns: ACTIVE_NS + 1_000_000_000,
            },
            recorded_at_ns: ACTIVE_NS,
        },
    );
    let permit = policy(
        &mut runtime,
        4,
        PairedPolicyCommand::AuthorizeFirst {
            command_id: PairedPolicyCommandId(bytes(4)),
            stage_id,
            leg_index: 0,
            max_mode_age_ns: 1_000_000_000,
            valid_until_ns: ACTIVE_NS + 50,
            recorded_at_ns: ACTIVE_NS,
        },
    )
    .policy_decision
    .expect("authorize")
    .permit
    .expect("permit");
    (runtime, stage_id, permit)
}

fn submit(
    runtime: &mut PairedPaperExecution,
    permit: &PairPermit,
    id: u8,
) -> PairedExecutionDecision {
    let recorded_at_ns = if permit.leg_index == 0 {
        ACTIVE_NS + 1
    } else {
        ACTIVE_NS + 5
    };
    runtime
        .apply(&PairedExecutionCommand::Submit {
            command_id: PairedExecutionCommandId(bytes(id)),
            permit: Box::new(permit.clone()),
            local_submission_id: format!("local-{id}"),
            recorded_at_ns,
        })
        .expect("submit")
}

fn observation(order_id: RiskOrderId, sequence: u64, event: ExchangeEvent) -> ExchangeObservation {
    ExchangeObservation {
        order_id,
        source_sequence: sequence,
        exchange_order_id: Some("exchange-1".into()),
        event,
        event_time_ns: ACTIVE_NS + i64::try_from(sequence).expect("sequence") + 1,
        received_time_ns: ACTIVE_NS + i64::try_from(sequence).expect("sequence") + 1,
    }
}

fn observe(
    runtime: &mut PairedPaperExecution,
    stage: PairStageId,
    leg: u8,
    id: u8,
    value: ExchangeObservation,
) -> Result<PairedExecutionDecision, Error> {
    let at = value.received_time_ns;
    runtime.apply(&PairedExecutionCommand::Observe {
        command_id: PairedExecutionCommandId(bytes(id)),
        stage_id: stage,
        leg_index: leg,
        observation: Box::new(value),
        recorded_at_ns: at,
    })
}

fn partial_fill(id: u8) -> MatchFill {
    MatchFill {
        fill_id: format!("fill-{id}"),
        quantity_micros: 500_000,
        consideration_micros: 200_000,
        fee_micros: 2_000,
        cumulative_quantity_micros: 500_000,
        cumulative_consideration_micros: 200_000,
        cumulative_fee_micros: 2_000,
        ledger_command_id: LedgerCommandId(bytes(id)),
    }
}
fn final_fill(id: u8) -> MatchFill {
    MatchFill {
        fill_id: format!("fill-{id}"),
        quantity_micros: 500_000,
        consideration_micros: 200_000,
        fee_micros: 2_000,
        cumulative_quantity_micros: 1_000_000,
        cumulative_consideration_micros: 400_000,
        cumulative_fee_micros: 4_000,
        ledger_command_id: LedgerCommandId(bytes(id)),
    }
}

#[test]
fn exact_permit_is_consumed_once_and_lifecycle_injection_is_blocked() {
    let (mut runtime, stage, permit) = setup();
    let accepted = submit(&mut runtime, &permit, 5);
    assert_eq!(accepted.status, PairedExecutionStatus::Applied);
    let duplicate = submit(&mut runtime, &permit, 6);
    assert_eq!(duplicate.reason, PairedExecutionReason::PermitAlreadyUsed);
    assert_eq!(runtime.snapshot().reserved_cash_micros, 910_000);
    let injected = PairedPolicyCommand::ObserveLeg {
        command_id: PairedPolicyCommandId(bytes(9)),
        stage_id: stage,
        leg_index: 0,
        permit_id: permit.permit_id,
        state: LegState::Unknown,
        source_sequence: 1,
        observed_at_ns: ACTIVE_NS + 2,
        recorded_at_ns: ACTIVE_NS + 2,
    };
    assert_eq!(
        runtime.apply(&PairedExecutionCommand::Policy {
            command_id: PairedExecutionCommandId(bytes(9)),
            command: Box::new(injected),
            recorded_at_ns: ACTIVE_NS + 2
        }),
        Err(Error::Boundary)
    );
    assert!(runtime.is_halted());
}

#[test]
fn partial_and_full_fills_emit_unique_handoffs_and_enable_hedge() {
    let (mut runtime, stage, permit) = setup();
    submit(&mut runtime, &permit, 5);
    let first = observe(
        &mut runtime,
        stage,
        0,
        6,
        observation(
            permit.order.order_id,
            1,
            ExchangeEvent::Match {
                fill: partial_fill(20),
                fully_matched: false,
            },
        ),
    )
    .expect("partial");
    assert!(first.new_handoff.is_some());
    assert_eq!(runtime.snapshot().handoff_count, 1);
    assert_eq!(runtime.snapshot().reserved_cash_micros, 910_000);
    let second = observe(
        &mut runtime,
        stage,
        0,
        7,
        observation(
            permit.order.order_id,
            2,
            ExchangeEvent::Match {
                fill: final_fill(21),
                fully_matched: true,
            },
        ),
    )
    .expect("full");
    assert!(second.new_handoff.is_some());
    assert_eq!(runtime.snapshot().handoff_count, 2);
    assert_eq!(runtime.snapshot().reserved_cash_micros, 910_000);
    let hedge = policy(
        &mut runtime,
        8,
        PairedPolicyCommand::AuthorizeHedge {
            command_id: PairedPolicyCommandId(bytes(8)),
            stage_id: stage,
            max_mode_age_ns: 1_000_000_000,
            valid_until_ns: ACTIVE_NS + 80,
            recorded_at_ns: ACTIVE_NS + 4,
        },
    )
    .policy_decision
    .expect("hedge")
    .permit
    .expect("permit");
    assert_eq!(hedge.leg_index, 1);
    assert_eq!(
        submit(&mut runtime, &hedge, 9).status,
        PairedExecutionStatus::Applied
    );
}

#[test]
fn unknown_cancel_and_no_fill_abort_preserve_then_release_pair() {
    let (mut runtime, stage, permit) = setup();
    submit(&mut runtime, &permit, 5);
    observe(
        &mut runtime,
        stage,
        0,
        6,
        observation(
            permit.order.order_id,
            1,
            ExchangeEvent::Unknown {
                reason: paper_execution::UnknownReason::SubmitTimeout,
            },
        ),
    )
    .expect("unknown");
    assert_eq!(runtime.snapshot().reserved_cash_micros, 910_000);
    let cancel = runtime
        .apply(&PairedExecutionCommand::RequestCancel {
            command_id: PairedExecutionCommandId(bytes(7)),
            stage_id: stage,
            leg_index: 0,
            recorded_at_ns: ACTIVE_NS + 3,
        })
        .expect("cancel");
    assert_eq!(cancel.status, PairedExecutionStatus::Applied);
    observe(
        &mut runtime,
        stage,
        0,
        8,
        observation(permit.order.order_id, 2, ExchangeEvent::CancelAccepted),
    )
    .expect("canceled");
    assert_eq!(runtime.snapshot().reserved_cash_micros, 910_000);
    let aborted = policy(
        &mut runtime,
        9,
        PairedPolicyCommand::AbortSafe {
            command_id: PairedPolicyCommandId(bytes(9)),
            stage_id: stage,
            recorded_at_ns: ACTIVE_NS + 5,
        },
    );
    assert_eq!(aborted.status, PairedExecutionStatus::Applied);
    assert_eq!(runtime.snapshot().reserved_cash_micros, 0);
}

#[test]
fn delayed_cancel_boundary_and_cancel_race_fill_are_exact() {
    let (mut runtime, stage, permit) = setup();
    submit(&mut runtime, &permit, 5);
    observe(
        &mut runtime,
        stage,
        0,
        6,
        observation(
            permit.order.order_id,
            1,
            ExchangeEvent::Delayed {
                release_at_ns: ACTIVE_NS + 10,
                uncancellable_until_ns: ACTIVE_NS + 12,
            },
        ),
    )
    .expect("delay");
    let early = runtime
        .apply(&PairedExecutionCommand::RequestCancel {
            command_id: PairedExecutionCommandId(bytes(7)),
            stage_id: stage,
            leg_index: 0,
            recorded_at_ns: ACTIVE_NS + 11,
        })
        .expect("early");
    assert_eq!(early.status, PairedExecutionStatus::Denied);
    let allowed = runtime
        .apply(&PairedExecutionCommand::RequestCancel {
            command_id: PairedExecutionCommandId(bytes(8)),
            stage_id: stage,
            leg_index: 0,
            recorded_at_ns: ACTIVE_NS + 12,
        })
        .expect("cancel");
    assert_eq!(allowed.status, PairedExecutionStatus::Applied);
    let mut fill = partial_fill(30);
    fill.quantity_micros = 1_000_000;
    fill.consideration_micros = 400_000;
    fill.fee_micros = 4_000;
    fill.cumulative_quantity_micros = 1_000_000;
    fill.cumulative_consideration_micros = 400_000;
    fill.cumulative_fee_micros = 4_000;
    let mut event = observation(
        permit.order.order_id,
        2,
        ExchangeEvent::Match {
            fill,
            fully_matched: true,
        },
    );
    event.event_time_ns = ACTIVE_NS + 13;
    event.received_time_ns = ACTIVE_NS + 13;
    let matched = observe(&mut runtime, stage, 0, 9, event).expect("race fill");
    assert!(matches!(matched.state, Some(OrderState::FullyMatched)));
    assert_eq!(runtime.snapshot().reserved_cash_micros, 910_000);
}

#[test]
fn invalid_fill_rolls_back_and_halts_without_releasing_reservations() {
    let (mut runtime, stage, permit) = setup();
    submit(&mut runtime, &permit, 5);
    let mut bad = partial_fill(40);
    bad.consideration_micros = 900_000;
    bad.cumulative_consideration_micros = 900_000;
    assert_eq!(
        observe(
            &mut runtime,
            stage,
            0,
            6,
            observation(
                permit.order.order_id,
                1,
                ExchangeEvent::Match {
                    fill: bad,
                    fully_matched: false
                }
            )
        ),
        Err(Error::FillInvariant)
    );
    assert!(runtime.is_halted());
    assert_eq!(
        runtime
            .order(stage, 0)
            .expect("order")
            .cumulative_quantity_micros,
        0
    );
    assert_eq!(runtime.snapshot().reserved_cash_micros, 910_000);
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

#[test]
fn durable_replay_checkpoint_and_sync_failure_are_fail_closed() {
    let directory = tempdir().expect("dir");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 256 * 1024,
            max_segment_records: 1,
        },
    )
    .expect("writer");
    let mut durable = DurablePairedExecution::new(
        writer,
        PairedExecutionRecovery {
            runtime: PairedPaperExecution::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    let fund = PairedExecutionCommand::Policy {
        command_id: PairedExecutionCommandId(bytes(1)),
        command: Box::new(PairedPolicyCommand::Fund {
            command_id: PairedPolicyCommandId(bytes(1)),
            amount_micros: 3_000_000,
            recorded_at_ns: ACTIVE_NS - 2,
        }),
        recorded_at_ns: ACTIVE_NS - 2,
    };
    durable.apply(&fund).expect("fund");
    let digest = durable.runtime().snapshot().digest;
    let checkpoint = PairedExecutionCheckpoint {
        sequence: 0,
        runtime_digest: digest,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("write");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    assert_eq!(
        recover_segmented(&segments, Some(checkpoint))
            .expect("recover")
            .runtime
            .snapshot()
            .digest,
        digest
    );
    let mut failing = DurablePairedExecution::new(
        FailingJournal::default(),
        PairedExecutionRecovery {
            runtime: PairedPaperExecution::default(),
            last_sequence: None,
        },
    )
    .expect("failing");
    assert!(matches!(
        failing.apply(&fund),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(
        failing
            .runtime()
            .policy()
            .staging()
            .ledger_risk_view()
            .cash_available_micros,
        0
    );
}

proptest! {
    #[test]
    fn any_nonterminal_or_matched_execution_state_requires_retention(state in prop_oneof![Just(ActiveState::Submitted), Just(ActiveState::Live), Just(ActiveState::PartiallyMatched), Just(ActiveState::Unknown { reason: paper_execution::UnknownReason::RecoveryRequired })]) {
        let order_state = OrderState::Active(state); prop_assert!(!order_state.is_terminal());
    }
}
