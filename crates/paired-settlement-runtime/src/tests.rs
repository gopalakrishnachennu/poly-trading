use super::*;
use accounting_ledger::{ConfirmedTokenBalance, ReservationStatus};
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
use paired_opportunity_runtime::{PairRiskFrame, PairedCommandId};
use paired_paper_execution::{PairedExecutionCommandId, PairedExecutionStatus};
use paired_placement_policy::{PairPermit, PairedPolicyCommand, PairedPolicyCommandId};
use paper_execution::{ExchangeEvent, ExchangeObservation, MatchFill};
use portfolio_risk::{BinaryMarketRisk, GroupMultiplier, RiskLimits, ShockProfile};
use proptest::prelude::*;
use public_market_data::{Asset, MarketIdentity, BTC_HOURLY};
use reference_market_data::{
    CandleData, CandleInterval, InProgressCandle, ReferenceHealth, ReferenceSnapshot,
    ReferenceSymbol,
};
use std::collections::BTreeMap;
use tempfile::tempdir;

const HOUR_MS: i64 = 3_600_000;
const ACTIVE_NS: i64 = HOUR_MS * 1_000_000;

const fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn config() -> ReconcilerConfig {
    ReconcilerConfig {
        chain_id: 137,
        wallet: "paper-wallet".into(),
        confirmation_grace_ns: 1_000,
        max_intents: 32,
        max_tokens: 8,
    }
}

#[allow(clippy::too_many_lines)]
fn context() -> strategy_proposal::StrategyContext {
    let identity = MarketIdentity {
        asset: Asset::Bitcoin,
        event_id: "event-a".into(),
        market_id: "market-a".into(),
        condition_id: format!("0x{}", "a".repeat(64)),
        question_id: format!("0x{}", "b".repeat(64)),
        event_slug: "event-a".into(),
        market_slug: "market-a".into(),
        series_id: BTC_HOURLY.id.into(),
        series_slug: BTC_HOURLY.slug.into(),
        title: "Up or Down".into(),
        start_time_ms: HOUR_MS,
        end_time_ms: 2 * HOUR_MS,
        resolution_source: "https://www.binance.com/en/trade/BTC_USDT".into(),
        description: "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the BTC/USDT 1 hour candle that begins on the time and date specified in the title. Otherwise, this market will resolve to \"Down\". The close and open displayed for the relevant candle will be used once the data for that candle is finalized. This market is about the Binance pair, not according to other exchanges or trading pairs.".into(),
        up_token_id: "up-a".into(),
        down_token_id: "down-a".into(),
        rules_fingerprint: bytes(7),
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

fn exact_chain(runtime: &PairedSettlementRuntime, block: u64, at: i64) -> FinalizedChainSnapshot {
    let view = runtime.ledger_reconciliation_view();
    FinalizedChainSnapshot {
        chain_id: 137,
        wallet: "paper-wallet".into(),
        block_number: block,
        block_hash: format!("block-{block}"),
        finalized_at_ns: at,
        observed_at_ns: at,
        collateral_micros: view.collateral_micros,
        token_balances: view
            .token_balances
            .into_iter()
            .map(|balance| settlement_reconciliation::ChainTokenBalance {
                token: balance.token,
                balance_micros: balance.balance_micros,
            })
            .collect(),
    }
}

fn run(
    runtime: &mut PairedSettlementRuntime,
    command: &PairedSettlementCommand,
) -> PairedSettlementOutcome {
    runtime.apply(command).expect("command")
}

fn execution(
    runtime: &mut PairedSettlementRuntime,
    outer_id: u8,
    command: PairedExecutionCommand,
) -> PairedExecutionDecision {
    let at = command.recorded_at_ns();
    let outcome = run(
        runtime,
        &PairedSettlementCommand::Execution {
            command_id: PairedSettlementCommandId(bytes(outer_id)),
            command: Box::new(command),
            recorded_at_ns: at,
        },
    );
    match outcome.detail {
        PairedSettlementDetail::Execution(decision) => *decision,
        other => panic!("unexpected {other:?}"),
    }
}

fn policy(
    runtime: &mut PairedSettlementRuntime,
    id: u8,
    command: PairedPolicyCommand,
) -> PairedExecutionDecision {
    let at = command.recorded_at_ns();
    execution(
        runtime,
        id,
        PairedExecutionCommand::Policy {
            command_id: PairedExecutionCommandId(bytes(id)),
            command: Box::new(command),
            recorded_at_ns: at,
        },
    )
}

fn paired(runtime: &PairedSettlementRuntime) -> PairedCommand {
    let context = context();
    let ledger = runtime.execution().policy().staging().ledger_risk_view();
    PairedCommand::Evaluate {
        command_id: PairedCommandId(bytes(33)),
        arbitrage_command: Box::new(ArbitrageCommand::Evaluate {
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
        }),
        risk_frame: Box::new(PairRiskFrame {
            reconciliation: runtime.reconciler().risk_gate(),
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

fn reconcile(runtime: &mut PairedSettlementRuntime, id: u8, block: u64, at: i64) {
    let chain = exact_chain(runtime, block, at);
    run(
        runtime,
        &PairedSettlementCommand::Reconcile {
            command_id: PairedSettlementCommandId(bytes(id)),
            chain,
            evaluated_at_ns: at,
            recorded_at_ns: at,
        },
    );
}

fn setup_stage() -> (PairedSettlementRuntime, PairStageId, PairPermit) {
    let mut runtime = PairedSettlementRuntime::new(config()).expect("runtime");
    policy(
        &mut runtime,
        1,
        PairedPolicyCommand::Fund {
            command_id: PairedPolicyCommandId(bytes(1)),
            amount_micros: 3_000_000,
            recorded_at_ns: ACTIVE_NS - 2,
        },
    );
    reconcile(&mut runtime, 2, 1, ACTIVE_NS - 1);
    let pair = paired(&runtime);
    let staged = policy(
        &mut runtime,
        3,
        PairedPolicyCommand::Stage {
            command_id: PairedPolicyCommandId(bytes(3)),
            paired_command: Box::new(pair),
            recorded_at_ns: ACTIVE_NS,
        },
    );
    let stage = staged
        .policy_decision
        .expect("staged")
        .stage_id
        .expect("stage");
    policy(
        &mut runtime,
        4,
        PairedPolicyCommand::ObserveMode {
            command_id: PairedPolicyCommandId(bytes(4)),
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
        5,
        PairedPolicyCommand::AuthorizeFirst {
            command_id: PairedPolicyCommandId(bytes(5)),
            stage_id: stage,
            leg_index: 0,
            max_mode_age_ns: 1_000_000_000,
            valid_until_ns: ACTIVE_NS + 50,
            recorded_at_ns: ACTIVE_NS,
        },
    )
    .policy_decision
    .expect("authorization")
    .permit
    .expect("permit");
    (runtime, stage, permit)
}

fn submit(runtime: &mut PairedSettlementRuntime, outer: u8, permit: &PairPermit, at: i64) {
    let result = execution(
        runtime,
        outer,
        PairedExecutionCommand::Submit {
            command_id: PairedExecutionCommandId(bytes(outer)),
            permit: Box::new(permit.clone()),
            local_submission_id: format!("local-{outer}"),
            recorded_at_ns: at,
        },
    );
    assert_eq!(result.status, PairedExecutionStatus::Applied);
}

fn fill_leg(
    runtime: &mut PairedSettlementRuntime,
    outer: u8,
    stage: PairStageId,
    permit: &PairPermit,
    consideration: i128,
    fee: i128,
    at: i64,
) {
    let fill = MatchFill {
        fill_id: format!("fill-{outer}"),
        quantity_micros: 1_000_000,
        consideration_micros: consideration,
        fee_micros: fee,
        cumulative_quantity_micros: 1_000_000,
        cumulative_consideration_micros: consideration,
        cumulative_fee_micros: fee,
        ledger_command_id: LedgerCommandId(bytes(outer)),
    };
    execution(
        runtime,
        outer,
        PairedExecutionCommand::Observe {
            command_id: PairedExecutionCommandId(bytes(outer)),
            stage_id: stage,
            leg_index: permit.leg_index,
            observation: Box::new(ExchangeObservation {
                order_id: permit.order.order_id,
                source_sequence: 1,
                exchange_order_id: Some(format!("exchange-{}", permit.leg_index)),
                event: ExchangeEvent::Match {
                    fill,
                    fully_matched: true,
                },
                event_time_ns: at,
                received_time_ns: at,
            }),
            recorded_at_ns: at,
        },
    );
}

fn setup_filled_pair() -> (PairedSettlementRuntime, PairStageId) {
    let (mut runtime, stage, first) = setup_stage();
    submit(&mut runtime, 6, &first, ACTIVE_NS + 1);
    fill_leg(
        &mut runtime,
        7,
        stage,
        &first,
        390_000,
        4_000,
        ACTIVE_NS + 2,
    );
    let hedge = policy(
        &mut runtime,
        8,
        PairedPolicyCommand::AuthorizeHedge {
            command_id: PairedPolicyCommandId(bytes(8)),
            stage_id: stage,
            max_mode_age_ns: 1_000_000_000,
            valid_until_ns: ACTIVE_NS + 80,
            recorded_at_ns: ACTIVE_NS + 3,
        },
    )
    .policy_decision
    .expect("hedge")
    .permit
    .expect("permit");
    submit(&mut runtime, 9, &hedge, ACTIVE_NS + 4);
    fill_leg(
        &mut runtime,
        10,
        stage,
        &hedge,
        490_000,
        6_000,
        ACTIVE_NS + 5,
    );
    (runtime, stage)
}

fn register(runtime: &mut PairedSettlementRuntime, id: u8, stage: PairStageId, leg: u8) {
    run(
        runtime,
        &PairedSettlementCommand::RegisterHandoff {
            command_id: PairedSettlementCommandId(bytes(id)),
            stage_id: stage,
            leg_index: leg,
            handoff_index: 0,
            recorded_at_ns: ACTIVE_NS + i64::from(id),
        },
    );
}

fn trade_observation(record: &RegisteredHandoff, status: TradeStatus, at: i64) -> TradeObservation {
    TradeObservation {
        trade_id: record.intent.trade_id.clone(),
        order_id: record.intent.order_id.clone(),
        token: record.intent.token.clone(),
        side: record.intent.side,
        quantity_micros: record.intent.quantity_micros,
        consideration_micros: record.intent.consideration_micros,
        fee_micros: record.intent.fee_micros,
        status,
        transaction_hash: if matches!(status, TradeStatus::Mined | TradeStatus::Confirmed) {
            Some(format!("tx-{}", record.leg_index))
        } else {
            None
        },
        matched_at_ns: ACTIVE_NS + 5,
        updated_at_ns: at,
    }
}

fn observe_status(
    runtime: &mut PairedSettlementRuntime,
    id: u8,
    ledger_id: LedgerCommandId,
    status: TradeStatus,
) {
    let at = ACTIVE_NS + 20 + i64::from(id);
    let observation = trade_observation(runtime.handoff(ledger_id).expect("handoff"), status, at);
    run(
        runtime,
        &PairedSettlementCommand::ObserveTrade {
            command_id: PairedSettlementCommandId(bytes(id)),
            observation,
            recorded_at_ns: at,
        },
    );
}

fn settle_pair(runtime: &mut PairedSettlementRuntime, stage: PairStageId) {
    register(runtime, 20, stage, 0);
    register(runtime, 21, stage, 1);
    for (offset, ledger_id) in [LedgerCommandId(bytes(7)), LedgerCommandId(bytes(10))]
        .into_iter()
        .enumerate()
    {
        let base = 30 + u8::try_from(offset * 4).expect("id");
        observe_status(runtime, base, ledger_id, TradeStatus::Matched);
        observe_status(runtime, base + 1, ledger_id, TradeStatus::Mined);
        observe_status(runtime, base + 2, ledger_id, TradeStatus::Confirmed);
        run(
            runtime,
            &PairedSettlementCommand::PostConfirmed {
                command_id: PairedSettlementCommandId(bytes(base + 3)),
                ledger_command_id: ledger_id,
                recorded_at_ns: ACTIVE_NS + 20 + i64::from(base + 3),
            },
        );
    }
}

#[test]
fn authentic_handoffs_progress_to_confirmed_exact_postings_only() {
    let (mut runtime, stage) = setup_filled_pair();
    register(&mut runtime, 20, stage, 0);
    let ledger_id = LedgerCommandId(bytes(7));
    observe_status(&mut runtime, 30, ledger_id, TradeStatus::Matched);
    let error = runtime
        .apply(&PairedSettlementCommand::PostConfirmed {
            command_id: PairedSettlementCommandId(bytes(31)),
            ledger_command_id: ledger_id,
            recorded_at_ns: ACTIVE_NS + 60,
        })
        .expect_err("matched is unconfirmed");
    assert_eq!(error, Error::Confirmation);
    assert!(runtime.is_halted());
    assert!(!runtime.handoff(ledger_id).expect("handoff").posted);
}

#[test]
fn duplicate_or_detached_handoff_index_halts_without_registration() {
    let (mut runtime, stage) = setup_filled_pair();
    let result = runtime.apply(&PairedSettlementCommand::RegisterHandoff {
        command_id: PairedSettlementCommandId(bytes(20)),
        stage_id: stage,
        leg_index: 0,
        handoff_index: 7,
        recorded_at_ns: ACTIVE_NS + 20,
    });
    assert_eq!(result, Err(Error::Handoff));
    assert_eq!(runtime.snapshot().registered_handoffs, 0);
    assert!(runtime.is_halted());
}

#[test]
fn confirmed_pair_locks_then_reconciles_and_releases_residuals() {
    let (mut runtime, stage) = setup_filled_pair();
    settle_pair(&mut runtime, stage);
    reconcile(&mut runtime, 50, 2, ACTIVE_NS + 200);
    assert!(runtime.reconciliation_is_current());
    run(
        &mut runtime,
        &PairedSettlementCommand::LockCompletePair {
            command_id: PairedSettlementCommandId(bytes(51)),
            stage_id: stage,
            lock_id: LockId(bytes(90)),
            quantity_micros: 1_000_000,
            recorded_at_ns: ACTIVE_NS + 201,
        },
    );
    assert!(!runtime.reconciliation_is_current());
    reconcile(&mut runtime, 52, 3, ACTIVE_NS + 202);
    run(
        &mut runtime,
        &PairedSettlementCommand::FinalizeStage {
            command_id: PairedSettlementCommandId(bytes(53)),
            stage_id: stage,
            recorded_at_ns: ACTIVE_NS + 203,
        },
    );
    let stage_record = runtime
        .execution()
        .policy()
        .staging()
        .stage_record(stage)
        .expect("stage");
    for reservation in stage_record.reservation_ids {
        assert_ne!(
            runtime
                .execution()
                .policy()
                .staging()
                .reservation(reservation)
                .expect("reservation")
                .status,
            ReservationStatus::Active
        );
    }
    assert_eq!(runtime.snapshot().finalized_stages, 1);
    assert_eq!(runtime.snapshot().locked_stages, 1);
}

#[test]
fn finalized_chain_mismatch_halts_without_claiming_readiness() {
    let (mut runtime, stage) = setup_filled_pair();
    settle_pair(&mut runtime, stage);
    let mut chain = exact_chain(&runtime, 2, ACTIVE_NS + 200);
    chain.collateral_micros += 1;
    let result = runtime.apply(&PairedSettlementCommand::Reconcile {
        command_id: PairedSettlementCommandId(bytes(50)),
        chain,
        evaluated_at_ns: ACTIVE_NS + 200,
        recorded_at_ns: ACTIVE_NS + 200,
    });
    assert!(matches!(result, Err(Error::Reconciliation(_))));
    assert!(runtime.is_halted());
    assert!(!runtime.reconciliation_is_current());
}

#[test]
fn failed_trade_remains_unposted_and_can_finalize_after_exact_reconciliation() {
    let (mut runtime, stage) = setup_filled_pair();
    register(&mut runtime, 20, stage, 0);
    register(&mut runtime, 21, stage, 1);
    for (id, ledger_id) in [
        (30, LedgerCommandId(bytes(7))),
        (40, LedgerCommandId(bytes(10))),
    ] {
        observe_status(&mut runtime, id, ledger_id, TradeStatus::Matched);
        observe_status(&mut runtime, id + 1, ledger_id, TradeStatus::Retrying);
        observe_status(&mut runtime, id + 2, ledger_id, TradeStatus::Failed);
    }
    reconcile(&mut runtime, 50, 2, ACTIVE_NS + 200);
    run(
        &mut runtime,
        &PairedSettlementCommand::FinalizeStage {
            command_id: PairedSettlementCommandId(bytes(51)),
            stage_id: stage,
            recorded_at_ns: ACTIVE_NS + 201,
        },
    );
    assert_eq!(runtime.snapshot().posted_handoffs, 0);
    assert_eq!(runtime.snapshot().finalized_stages, 1);
}

#[test]
fn finalization_requires_terminal_trades_and_current_ledger_digest() {
    let (mut runtime, stage) = setup_filled_pair();
    register(&mut runtime, 20, stage, 0);
    register(&mut runtime, 21, stage, 1);
    let result = runtime.apply(&PairedSettlementCommand::FinalizeStage {
        command_id: PairedSettlementCommandId(bytes(22)),
        stage_id: stage,
        recorded_at_ns: ACTIVE_NS + 22,
    });
    assert_eq!(result, Err(Error::ReconciliationNotCurrent));
    assert!(runtime.is_halted());
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
    let mut durable = DurablePairedSettlement::new(
        writer,
        PairedSettlementRecovery {
            runtime: PairedSettlementRuntime::new(config()).expect("runtime"),
            last_sequence: None,
        },
    )
    .expect("durable");
    let command = PairedSettlementCommand::Execution {
        command_id: PairedSettlementCommandId(bytes(1)),
        command: Box::new(PairedExecutionCommand::Policy {
            command_id: PairedExecutionCommandId(bytes(1)),
            command: Box::new(PairedPolicyCommand::Fund {
                command_id: PairedPolicyCommandId(bytes(1)),
                amount_micros: 3_000_000,
                recorded_at_ns: ACTIVE_NS,
            }),
            recorded_at_ns: ACTIVE_NS,
        }),
        recorded_at_ns: ACTIVE_NS,
    };
    durable.apply(&command).expect("apply");
    let checkpoint = PairedSettlementCheckpoint {
        sequence: 0,
        runtime_digest: durable.runtime().snapshot().digest,
    };
    let checkpoint_path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&checkpoint_path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, config(), Some(checkpoint)).expect("recover");
    assert_eq!(
        recovered.runtime.snapshot().digest,
        checkpoint.runtime_digest
    );

    let mut failing = DurablePairedSettlement::new(
        FailingJournal::default(),
        PairedSettlementRecovery {
            runtime: PairedSettlementRuntime::new(config()).expect("runtime"),
            last_sequence: None,
        },
    )
    .expect("failing");
    assert!(matches!(
        failing.apply(&command),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(failing.runtime().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&command),
        Err(StorageError::Halted(_))
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]
    #[test]
    fn unconfirmed_status_never_posts(status_index in 0_u8..4) {
        let (mut runtime, stage) = setup_filled_pair();
        register(&mut runtime, 20, stage, 0);
        let ledger_id = LedgerCommandId(bytes(7));
        observe_status(&mut runtime, 30, ledger_id, TradeStatus::Matched);
        match status_index {
            0 => {}
            1 => observe_status(&mut runtime, 31, ledger_id, TradeStatus::Mined),
            2 => observe_status(&mut runtime, 31, ledger_id, TradeStatus::Retrying),
            3 => {
                observe_status(&mut runtime, 31, ledger_id, TradeStatus::Retrying);
                observe_status(&mut runtime, 32, ledger_id, TradeStatus::Failed);
            }
            _ => unreachable!(),
        }
        let result = runtime.apply(&PairedSettlementCommand::PostConfirmed {
            command_id: PairedSettlementCommandId(bytes(40)),
            ledger_command_id: ledger_id,
            recorded_at_ns: ACTIVE_NS + 100,
        });
        prop_assert_eq!(result, Err(Error::Confirmation));
        prop_assert!(!runtime.handoff(ledger_id).expect("handoff").posted);
    }
}

#[test]
fn invalid_config_is_rejected() {
    let result = PairedSettlementRuntime::new(ReconcilerConfig {
        chain_id: 0,
        wallet: String::new(),
        confirmation_grace_ns: 0,
        max_intents: 0,
        max_tokens: 0,
    });
    assert_eq!(result.expect_err("invalid"), Error::Config);
}

#[test]
fn codec_and_content_idempotency_are_strict() {
    let mut runtime = PairedSettlementRuntime::new(config()).expect("runtime");
    let command = PairedSettlementCommand::Execution {
        command_id: PairedSettlementCommandId(bytes(1)),
        command: Box::new(PairedExecutionCommand::Policy {
            command_id: PairedExecutionCommandId(bytes(1)),
            command: Box::new(PairedPolicyCommand::Fund {
                command_id: PairedPolicyCommandId(bytes(1)),
                amount_micros: 3_000_000,
                recorded_at_ns: ACTIVE_NS,
            }),
            recorded_at_ns: ACTIVE_NS,
        }),
        recorded_at_ns: ACTIVE_NS,
    };
    let encoded = encode_command(&command).expect("encode");
    assert_eq!(decode_command(&encoded).expect("decode"), command);
    let first = runtime.apply(&command).expect("first");
    assert_eq!(runtime.apply(&command).expect("duplicate"), first);

    let mut conflict = command;
    if let PairedSettlementCommand::Execution { command, .. } = &mut conflict {
        if let PairedExecutionCommand::Policy { command, .. } = command.as_mut() {
            if let PairedPolicyCommand::Fund { amount_micros, .. } = command.as_mut() {
                *amount_micros += 1;
            }
        }
    }
    assert_eq!(runtime.apply(&conflict), Err(Error::IdempotencyConflict));
    assert!(runtime.is_halted());
}

#[test]
fn staging_requires_reconciliation_for_the_current_nested_ledger() {
    let mut runtime = PairedSettlementRuntime::new(config()).expect("runtime");
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
    let command = PairedSettlementCommand::Execution {
        command_id: PairedSettlementCommandId(bytes(2)),
        command: Box::new(PairedExecutionCommand::Policy {
            command_id: PairedExecutionCommandId(bytes(2)),
            command: Box::new(PairedPolicyCommand::Stage {
                command_id: PairedPolicyCommandId(bytes(2)),
                paired_command: Box::new(pair),
                recorded_at_ns: ACTIVE_NS,
            }),
            recorded_at_ns: ACTIVE_NS,
        }),
        recorded_at_ns: ACTIVE_NS,
    };
    assert_eq!(runtime.apply(&command), Err(Error::Boundary));
    assert!(runtime.is_halted());
}

#[test]
fn reconciliation_view_preserves_confirmed_token_identity() {
    let (mut runtime, stage) = setup_filled_pair();
    settle_pair(&mut runtime, stage);
    let tokens: Vec<ConfirmedTokenBalance> = runtime.ledger_reconciliation_view().token_balances;
    assert_eq!(tokens.len(), 2);
    assert!(tokens.iter().all(|token| token.balance_micros == 1_000_000));
}
