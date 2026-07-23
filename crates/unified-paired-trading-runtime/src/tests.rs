use super::*;
use common_types::{PriceMicros, QuantityMicros, QuotePriceMicros, ReferenceQuantityE8};
use complete_set_arbitrage::{ArbitrageConstraints, ArbitrageDirection, ArbitrageEvaluationId};
use ctf_transaction_runtime::{ConversionEvent, ConversionState};
use feed_supervisor::{SupervisorMode, SupervisorSnapshot};
use live_market_state::{ActorMode, ActorSnapshot};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use market_session::{
    CoordinationFrame, MarketSessionCoordinator, SessionKey, SessionSourceState, TokenBookView,
};
use order_intent_policy::ExchangeMode;
use paired_capital_staging::PairStageStatus;
use portfolio_risk::{GroupMultiplier, RiskLimits};
use proptest::prelude::*;
use public_market_data::{Asset, MarketIdentity, BTC_HOURLY};
use reference_market_data::{
    CandleData, CandleInterval, InProgressCandle, ReferenceHealth, ReferenceSnapshot,
    ReferenceSymbol,
};
use settlement_reconciliation::{ChainTokenBalance, Side, TradeStatus};
use std::collections::BTreeMap;
use tempfile::tempdir;

const HOUR_MS: i64 = 3_600_000;
const HOUR_NS: i64 = 3_600_000_000_000;

const fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn config() -> ReconcilerConfig {
    ReconcilerConfig {
        chain_id: 137,
        wallet: "paper-wallet".into(),
        confirmation_grace_ns: 1_000,
        max_intents: 64,
        max_tokens: 16,
    }
}

#[allow(clippy::too_many_lines)]
fn context(hour: u8) -> strategy_proposal::StrategyContext {
    let start_ms = i64::from(hour) * HOUR_MS;
    let now_ns = i64::from(hour) * HOUR_NS;
    let suffix = hour.to_string();
    let identity = MarketIdentity {
        asset: Asset::Bitcoin,
        event_id: format!("event-{suffix}"),
        market_id: format!("market-{suffix}"),
        condition_id: format!("0x{}", format!("{hour:x}").repeat(64).chars().take(64).collect::<String>()),
        question_id: format!("0x{}", format!("{:x}", hour.saturating_add(1)).repeat(64).chars().take(64).collect::<String>()),
        event_slug: format!("event-{suffix}"),
        market_slug: format!("market-{suffix}"),
        series_id: BTC_HOURLY.id.into(),
        series_slug: BTC_HOURLY.slug.into(),
        title: "Up or Down".into(),
        start_time_ms: start_ms,
        end_time_ms: start_ms + HOUR_MS,
        resolution_source: "https://www.binance.com/en/trade/BTC_USDT".into(),
        description: "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the BTC/USDT 1 hour candle that begins on the time and date specified in the title. Otherwise, this market will resolve to \"Down\". The close and open displayed for the relevant candle will be used once the data for that candle is finalized. This market is about the Binance pair, not according to other exchanges or trading pairs.".into(),
        up_token_id: format!("up-{suffix}"),
        down_token_id: format!("down-{suffix}"),
        rules_fingerprint: bytes(hour),
    };
    let book = |bid, ask| TokenBookView {
        authoritative: true,
        best_bid: Some((
            PriceMicros::new(bid).expect("bid"),
            QuantityMicros::new(2_000_000).expect("quantity"),
        )),
        best_ask: Some((
            PriceMicros::new(ask).expect("ask"),
            QuantityMicros::new(2_000_000).expect("quantity"),
        )),
    };
    let market = ActorSnapshot {
        mode: ActorMode::Ready,
        ready: true,
        epoch: u64::from(hour),
        last_sequence: Some(10),
        book_count: 2,
        digest: bytes(3),
        last_market_event_ns: Some(now_ns),
        last_market_received_ns: Some(now_ns),
        halt_reason: None,
    };
    let reference = ReferenceSnapshot {
        health: ReferenceHealth::Ready,
        epoch: u64::from(hour),
        last_sequence: Some(20),
        digest: bytes(4),
        last_reference_received_ns: Some(now_ns),
        symbols: BTreeMap::new(),
    };
    let supervision = SupervisorSnapshot {
        mode: SupervisorMode::Ready,
        ready: true,
        evaluated_at_ns: Some(now_ns),
        market_epoch: u64::from(hour),
        market_sequence: Some(10),
        market_digest: bytes(5),
        market_state_digest: bytes(3),
        reference_epoch: u64::from(hour),
        reference_sequence: Some(20),
        reference_digest: bytes(6),
        reference_state_digest: bytes(4),
        halt_reason: None,
        digest: bytes(8),
    };
    let candle = CandleData {
        symbol: ReferenceSymbol::BtcUsdt,
        interval: CandleInterval::OneHourUtc,
        open_time_ms: start_ms,
        close_time_ms: start_ms + HOUR_MS - 1,
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
        now_ns,
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
    strategy_proposal::capture_context(&snapshot, &frame, &identity, now_ns, now_ns + 1_000)
        .expect("context")
}

fn evaluation(hour: u8, min_profit: i128) -> (ArbitrageRequest, EvaluationRiskInputs) {
    let now = i64::from(hour) * HOUR_NS;
    let context = context(hour);
    let markets = vec![BinaryMarketRisk {
        condition_id: context.condition_id.clone(),
        up: context.up_token.clone(),
        down: context.down_token.clone(),
        shock_group: "crypto".into(),
    }];
    (
        ArbitrageRequest {
            evaluation_id: ArbitrageEvaluationId(bytes(hour)),
            direction: ArbitrageDirection::BuyPair,
            context: Box::new(context),
            constraints: ArbitrageConstraints {
                min_quantity_micros: 100_000,
                max_quantity_micros: 1_000_000,
                partial_fill_micros: 50_000,
                up_max_fee_micros: 4_000,
                down_max_fee_micros: 6_000,
                conversion_max_cost_micros: 1_000,
                min_net_profit_micros: min_profit,
                min_roi_bps: 1,
            },
            evaluated_at_ns: now,
            expires_at_ns: now + 100,
        },
        EvaluationRiskInputs {
            markets,
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
                max_reconciliation_age_ns: HOUR_NS,
                max_open_orders: 4,
                max_scenarios: 1_000,
            },
        },
    )
}

#[allow(clippy::needless_pass_by_value)]
fn run(runtime: &mut UnifiedPairedTradingRuntime, command: UnifiedCommand) -> UnifiedOutcome {
    runtime.apply(&command).expect("command")
}

fn exact_chain(
    runtime: &UnifiedPairedTradingRuntime,
    block: u64,
    at: i64,
) -> FinalizedChainSnapshot {
    let view = runtime.ctf().parent().ledger_reconciliation_view();
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
            .map(|balance| ChainTokenBalance {
                token: balance.token,
                balance_micros: balance.balance_micros,
            })
            .collect(),
    }
}

fn fund_and_reconcile(runtime: &mut UnifiedPairedTradingRuntime, hour: u8) {
    let now = i64::from(hour) * HOUR_NS;
    run(
        runtime,
        UnifiedCommand::Fund {
            command_id: UnifiedCommandId(bytes(1)),
            amount_micros: 3_000_000,
            recorded_at_ns: now - 2,
        },
    );
    let chain = exact_chain(runtime, 1, now - 1);
    run(
        runtime,
        UnifiedCommand::Reconcile {
            command_id: UnifiedCommandId(bytes(2)),
            chain,
            evaluated_at_ns: now - 1,
            recorded_at_ns: now - 1,
        },
    );
}

fn stage(runtime: &mut UnifiedPairedTradingRuntime, hour: u8) -> PairStageId {
    let now = i64::from(hour) * HOUR_NS;
    let (request, risk) = evaluation(hour, 1);
    let outcome = run(
        runtime,
        UnifiedCommand::EvaluateAndStage {
            command_id: UnifiedCommandId(bytes(3)),
            request: Box::new(request),
            risk: Box::new(risk),
            recorded_at_ns: now,
        },
    );
    let UnifiedDetail::Staged { stage_id } = outcome.detail else {
        panic!("expected stage")
    };
    stage_id
}

fn observe_full_match(
    runtime: &mut UnifiedPairedTradingRuntime,
    command: u8,
    stage_id: PairStageId,
    leg_index: u8,
    consideration: i128,
    fee: i128,
    at: i64,
) {
    let outcome = run(
        runtime,
        UnifiedCommand::ObserveOrder {
            command_id: UnifiedCommandId(bytes(command)),
            stage_id,
            leg_index,
            observation: Box::new(UnifiedOrderObservation {
                source_sequence: 1,
                exchange_order_id: Some(format!("exchange-{leg_index}")),
                event: UnifiedOrderEvent::Match {
                    fill_id: format!("fill-{leg_index}"),
                    quantity_micros: 1_000_000,
                    consideration_micros: consideration,
                    fee_micros: fee,
                    cumulative_quantity_micros: 1_000_000,
                    cumulative_consideration_micros: consideration,
                    cumulative_fee_micros: fee,
                    fully_matched: true,
                },
                event_time_ns: at,
                received_time_ns: at,
            }),
            recorded_at_ns: at,
        },
    );
    assert_eq!(
        outcome.detail,
        UnifiedDetail::OrderObserved {
            handoff_created: true
        }
    );
}

fn setup_filled_pair() -> (UnifiedPairedTradingRuntime, PairStageId, i64) {
    let hour = 1;
    let now = i64::from(hour) * HOUR_NS;
    let mut runtime = UnifiedPairedTradingRuntime::new(config()).expect("runtime");
    fund_and_reconcile(&mut runtime, hour);
    let stage_id = stage(&mut runtime, hour);
    run(
        &mut runtime,
        UnifiedCommand::ObserveMode {
            command_id: UnifiedCommandId(bytes(4)),
            observation: ExchangeModeObservation {
                sequence: 1,
                mode: ExchangeMode::Normal,
                observed_at_ns: now,
                valid_until_ns: now + 1_000_000_000,
            },
            recorded_at_ns: now,
        },
    );
    run(
        &mut runtime,
        UnifiedCommand::AuthorizeAndSubmitFirst {
            command_id: UnifiedCommandId(bytes(5)),
            stage_id,
            leg_index: 0,
            max_mode_age_ns: 1_000_000_000,
            valid_until_ns: now + 50,
            local_submission_id: "local-first".into(),
            recorded_at_ns: now + 1,
        },
    );
    observe_full_match(&mut runtime, 6, stage_id, 0, 390_000, 4_000, now + 2);
    run(
        &mut runtime,
        UnifiedCommand::AuthorizeAndSubmitHedge {
            command_id: UnifiedCommandId(bytes(7)),
            stage_id,
            max_mode_age_ns: 1_000_000_000,
            valid_until_ns: now + 80,
            local_submission_id: "local-hedge".into(),
            recorded_at_ns: now + 3,
        },
    );
    observe_full_match(&mut runtime, 8, stage_id, 1, 490_000, 6_000, now + 4);
    (runtime, stage_id, now)
}

fn register(
    runtime: &mut UnifiedPairedTradingRuntime,
    id: u8,
    stage: PairStageId,
    leg: u8,
    at: i64,
) {
    run(
        runtime,
        UnifiedCommand::RegisterHandoff {
            command_id: UnifiedCommandId(bytes(id)),
            stage_id: stage,
            leg_index: leg,
            handoff_index: 0,
            recorded_at_ns: at,
        },
    );
}

fn trade_for(
    runtime: &UnifiedPairedTradingRuntime,
    stage: PairStageId,
    leg: u8,
    status: TradeStatus,
    matched_at: i64,
    at: i64,
) -> TradeObservation {
    let order = runtime
        .ctf()
        .parent()
        .execution()
        .order(stage, leg)
        .expect("order");
    let intent = &order.handoffs[0].intent;
    TradeObservation {
        trade_id: intent.trade_id.clone(),
        order_id: intent.order_id.clone(),
        token: intent.token.clone(),
        side: if order.permit.order.side == portfolio_risk::OrderSide::Buy {
            Side::Buy
        } else {
            Side::Sell
        },
        quantity_micros: intent.quantity_micros,
        consideration_micros: intent.consideration_micros,
        fee_micros: intent.fee_micros,
        status,
        transaction_hash: if matches!(status, TradeStatus::Mined | TradeStatus::Confirmed) {
            Some(format!("tx-{leg}"))
        } else {
            None
        },
        matched_at_ns: matched_at,
        updated_at_ns: at,
    }
}

fn settle_leg(
    runtime: &mut UnifiedPairedTradingRuntime,
    stage: PairStageId,
    leg: u8,
    base_id: u8,
    base_at: i64,
) {
    for (offset, status) in [
        TradeStatus::Matched,
        TradeStatus::Mined,
        TradeStatus::Confirmed,
    ]
    .into_iter()
    .enumerate()
    {
        let at = base_at + i64::try_from(offset).expect("offset");
        let observation = trade_for(runtime, stage, leg, status, base_at - 3, at);
        run(
            runtime,
            UnifiedCommand::ObserveTrade {
                command_id: UnifiedCommandId(bytes(base_id + u8::try_from(offset).expect("id"))),
                observation,
                recorded_at_ns: at,
            },
        );
    }
    run(
        runtime,
        UnifiedCommand::PostConfirmed {
            command_id: UnifiedCommandId(bytes(base_id + 3)),
            stage_id: stage,
            leg_index: leg,
            handoff_index: 0,
            recorded_at_ns: base_at + 3,
        },
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn complete_pair_runs_from_evaluation_to_confirmed_merge_and_finalization() {
    let (mut runtime, stage_id, now) = setup_filled_pair();
    register(&mut runtime, 9, stage_id, 0, now + 9);
    register(&mut runtime, 10, stage_id, 1, now + 10);
    settle_leg(&mut runtime, stage_id, 0, 11, now + 11);
    settle_leg(&mut runtime, stage_id, 1, 15, now + 15);
    let chain = exact_chain(&runtime, 2, now + 20);
    run(
        &mut runtime,
        UnifiedCommand::Reconcile {
            command_id: UnifiedCommandId(bytes(19)),
            chain,
            evaluated_at_ns: now + 20,
            recorded_at_ns: now + 20,
        },
    );
    let lock_id = LockId(bytes(90));
    run(
        &mut runtime,
        UnifiedCommand::LockCompletePair {
            command_id: UnifiedCommandId(bytes(20)),
            stage_id,
            lock_id,
            quantity_micros: 1_000_000,
            recorded_at_ns: now + 21,
        },
    );
    let chain = exact_chain(&runtime, 3, now + 22);
    run(
        &mut runtime,
        UnifiedCommand::Reconcile {
            command_id: UnifiedCommandId(bytes(21)),
            chain,
            evaluated_at_ns: now + 22,
            recorded_at_ns: now + 22,
        },
    );
    let stage = runtime
        .ctf()
        .parent()
        .execution()
        .policy()
        .staging()
        .stage_record(stage_id)
        .expect("stage")
        .clone();
    let conversion_id = ConversionId(bytes(91));
    run(
        &mut runtime,
        UnifiedCommand::RequestConversion {
            command_id: UnifiedCommandId(bytes(22)),
            conversion_id,
            request: ConversionRequest::Merge {
                lock_id,
                up: stage.candidates[0].token.clone(),
                down: stage.candidates[1].token.clone(),
                quantity_micros: 1_000_000,
            },
            recorded_at_ns: now + 23,
        },
    );
    for (command, sequence, event) in [
        (
            23,
            1,
            ConversionEvent::Pending {
                external_transaction_id: "merge-1".into(),
            },
        ),
        (
            24,
            2,
            ConversionEvent::Confirmed {
                transaction_hash: "merge-hash".into(),
            },
        ),
    ] {
        let at = now + i64::from(command) + 1;
        run(
            &mut runtime,
            UnifiedCommand::ObserveConversion {
                command_id: UnifiedCommandId(bytes(command)),
                observation: ConversionObservation {
                    conversion_id,
                    source_sequence: sequence,
                    event,
                    event_time_ns: at,
                    received_time_ns: at,
                },
                recorded_at_ns: at,
            },
        );
    }
    assert!(matches!(
        runtime
            .ctf()
            .record(conversion_id)
            .expect("conversion")
            .state,
        ConversionState::Confirmed { .. }
    ));
    let chain = exact_chain(&runtime, 4, now + 30);
    run(
        &mut runtime,
        UnifiedCommand::Reconcile {
            command_id: UnifiedCommandId(bytes(25)),
            chain,
            evaluated_at_ns: now + 30,
            recorded_at_ns: now + 30,
        },
    );
    run(
        &mut runtime,
        UnifiedCommand::FinalizeStage {
            command_id: UnifiedCommandId(bytes(26)),
            stage_id,
            recorded_at_ns: now + 31,
        },
    );
    assert_eq!(runtime.snapshot().cash_available_micros, 3_110_000);
    assert_eq!(runtime.snapshot().cash_reserved_micros, 0);
    assert_eq!(
        runtime
            .ctf()
            .parent()
            .execution()
            .policy()
            .staging()
            .stage_record(stage_id)
            .expect("stage")
            .status,
        PairStageStatus::FullyReserved
    );
    assert_eq!(runtime.ctf().parent().snapshot().finalized_stages, 1);
}

#[test]
#[allow(clippy::too_many_lines)]
fn split_and_merge_retain_backing_and_restore_capital() {
    let hour = 1;
    let now = i64::from(hour) * HOUR_NS;
    let mut runtime = UnifiedPairedTradingRuntime::new(config()).expect("runtime");
    fund_and_reconcile(&mut runtime, hour);
    let up = accounting_ledger::TokenKey::new("split-condition", "up").expect("up");
    let down = accounting_ledger::TokenKey::new("split-condition", "down").expect("down");
    let split_id = ConversionId(bytes(40));
    run(
        &mut runtime,
        UnifiedCommand::RequestConversion {
            command_id: UnifiedCommandId(bytes(3)),
            conversion_id: split_id,
            request: ConversionRequest::Split {
                up: up.clone(),
                down: down.clone(),
                quantity_micros: 1_000_000,
            },
            recorded_at_ns: now,
        },
    );
    assert_eq!(runtime.snapshot().cash_reserved_micros, 1_000_000);
    for (command, sequence, event) in [
        (
            4,
            1,
            ConversionEvent::Pending {
                external_transaction_id: "split-1".into(),
            },
        ),
        (
            5,
            2,
            ConversionEvent::Retrying {
                reason: "offline retry observation".into(),
            },
        ),
        (
            6,
            3,
            ConversionEvent::Confirmed {
                transaction_hash: "split-hash".into(),
            },
        ),
    ] {
        let at = now + i64::from(command);
        run(
            &mut runtime,
            UnifiedCommand::ObserveConversion {
                command_id: UnifiedCommandId(bytes(command)),
                observation: ConversionObservation {
                    conversion_id: split_id,
                    source_sequence: sequence,
                    event,
                    event_time_ns: at,
                    received_time_ns: at,
                },
                recorded_at_ns: at,
            },
        );
    }
    let chain = exact_chain(&runtime, 2, now + 7);
    run(
        &mut runtime,
        UnifiedCommand::Reconcile {
            command_id: UnifiedCommandId(bytes(7)),
            chain,
            evaluated_at_ns: now + 7,
            recorded_at_ns: now + 7,
        },
    );
    let merge_id = ConversionId(bytes(41));
    run(
        &mut runtime,
        UnifiedCommand::RequestConversion {
            command_id: UnifiedCommandId(bytes(8)),
            conversion_id: merge_id,
            request: ConversionRequest::Merge {
                lock_id: LockId(bytes(42)),
                up,
                down,
                quantity_micros: 1_000_000,
            },
            recorded_at_ns: now + 8,
        },
    );
    for (command, sequence, event) in [
        (
            9,
            1,
            ConversionEvent::Pending {
                external_transaction_id: "merge-2".into(),
            },
        ),
        (
            10,
            2,
            ConversionEvent::Confirmed {
                transaction_hash: "merge-2-hash".into(),
            },
        ),
    ] {
        let at = now + i64::from(command);
        run(
            &mut runtime,
            UnifiedCommand::ObserveConversion {
                command_id: UnifiedCommandId(bytes(command)),
                observation: ConversionObservation {
                    conversion_id: merge_id,
                    source_sequence: sequence,
                    event,
                    event_time_ns: at,
                    received_time_ns: at,
                },
                recorded_at_ns: at,
            },
        );
    }
    assert_eq!(runtime.snapshot().cash_available_micros, 3_000_000);
    assert_eq!(runtime.snapshot().cash_reserved_micros, 0);
    assert_eq!(runtime.snapshot().confirmed_conversion_count, 2);
}

#[test]
fn injected_authorization_submission_boundary_is_transactional_and_absorbing() {
    let hour = 1;
    let now = i64::from(hour) * HOUR_NS;
    let mut runtime = UnifiedPairedTradingRuntime::new(config()).expect("runtime");
    fund_and_reconcile(&mut runtime, hour);
    let stage_id = stage(&mut runtime, hour);
    run(
        &mut runtime,
        UnifiedCommand::ObserveMode {
            command_id: UnifiedCommandId(bytes(4)),
            observation: ExchangeModeObservation {
                sequence: 1,
                mode: ExchangeMode::Normal,
                observed_at_ns: now,
                valid_until_ns: now + 1_000_000_000,
            },
            recorded_at_ns: now,
        },
    );
    run(
        &mut runtime,
        UnifiedCommand::InjectFault {
            command_id: UnifiedCommandId(bytes(5)),
            fault: UnifiedFault::AfterAuthorizationBeforeSubmission,
            recorded_at_ns: now,
        },
    );
    let before = runtime.ctf().snapshot().digest;
    let result = runtime.apply(&UnifiedCommand::AuthorizeAndSubmitFirst {
        command_id: UnifiedCommandId(bytes(6)),
        stage_id,
        leg_index: 0,
        max_mode_age_ns: 1_000_000_000,
        valid_until_ns: now + 50,
        local_submission_id: "must-rollback".into(),
        recorded_at_ns: now + 1,
    });
    assert_eq!(result, Err(Error::InjectedFault));
    assert_eq!(runtime.ctf().snapshot().digest, before);
    assert!(runtime.is_halted());
    assert!(runtime
        .ctf()
        .parent()
        .execution()
        .order(stage_id, 0)
        .is_none());
}

#[test]
fn command_content_idempotency_is_exact_and_conflicts_halt() {
    let now = HOUR_NS;
    let mut runtime = UnifiedPairedTradingRuntime::new(config()).expect("runtime");
    let command = UnifiedCommand::Fund {
        command_id: UnifiedCommandId(bytes(1)),
        amount_micros: 3_000_000,
        recorded_at_ns: now,
    };
    let first = runtime.apply(&command).expect("first");
    assert_eq!(runtime.apply(&command), Ok(first));
    assert_eq!(runtime.snapshot().cash_available_micros, 3_000_000);
    assert_eq!(
        runtime.apply(&UnifiedCommand::Fund {
            command_id: UnifiedCommandId(bytes(1)),
            amount_micros: 3_000_001,
            recorded_at_ns: now,
        }),
        Err(Error::IdempotencyConflict)
    );
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
            market_recorder::JournalError::Io(std::io::Error::other("sync failure")),
        ))
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}

fn fund_command(now: i64) -> UnifiedCommand {
    UnifiedCommand::Fund {
        command_id: UnifiedCommandId(bytes(1)),
        amount_micros: 3_000_000,
        recorded_at_ns: now,
    }
}

#[test]
fn codec_journal_checkpoint_restart_and_sync_failure_are_fail_closed() {
    let now = HOUR_NS;
    let command = fund_command(now);
    assert_eq!(
        decode_command(&encode_command(&command).expect("encode")).expect("decode"),
        command
    );
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
    let mut durable = DurableUnifiedRuntime::new(
        writer,
        UnifiedRecovery {
            runtime: UnifiedPairedTradingRuntime::new(config()).expect("runtime"),
            last_sequence: None,
        },
    )
    .expect("durable");
    durable.apply(&command).expect("apply");
    let chain = exact_chain(durable.runtime(), 1, now + 1);
    durable
        .apply(&UnifiedCommand::Reconcile {
            command_id: UnifiedCommandId(bytes(2)),
            chain,
            evaluated_at_ns: now + 1,
            recorded_at_ns: now + 1,
        })
        .expect("reconcile");
    let conversion_id = ConversionId(bytes(20));
    durable
        .apply(&UnifiedCommand::RequestConversion {
            command_id: UnifiedCommandId(bytes(3)),
            conversion_id,
            request: ConversionRequest::Split {
                up: accounting_ledger::TokenKey::new("restart", "up").expect("up"),
                down: accounting_ledger::TokenKey::new("restart", "down").expect("down"),
                quantity_micros: 1_000_000,
            },
            recorded_at_ns: now + 2,
        })
        .expect("request");
    durable
        .apply(&UnifiedCommand::ObserveConversion {
            command_id: UnifiedCommandId(bytes(4)),
            observation: ConversionObservation {
                conversion_id,
                source_sequence: 1,
                event: ConversionEvent::Pending {
                    external_transaction_id: "restart-pending".into(),
                },
                event_time_ns: now + 3,
                received_time_ns: now + 3,
            },
            recorded_at_ns: now + 3,
        })
        .expect("pending");
    let checkpoint = UnifiedCheckpoint {
        sequence: 3,
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
    assert_eq!(recovered.runtime.snapshot().pending_conversion_count, 1);
    assert_eq!(recovered.runtime.snapshot().cash_reserved_micros, 1_000_000);

    let mut failing = DurableUnifiedRuntime::new(
        FailingJournal::default(),
        UnifiedRecovery {
            runtime: UnifiedPairedTradingRuntime::new(config()).expect("runtime"),
            last_sequence: None,
        },
    )
    .expect("failing");
    assert!(matches!(
        failing.apply(&command),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(failing.runtime().snapshot().cash_available_micros, 0);
    assert!(matches!(
        failing.apply(&command),
        Err(StorageError::Halted(_))
    ));
}

#[test]
fn twenty_four_no_trade_hours_do_not_reserve_or_move_capital() {
    let mut runtime = UnifiedPairedTradingRuntime::new(config()).expect("runtime");
    fund_and_reconcile(&mut runtime, 1);
    for hour in 1_u8..=24 {
        let now = i64::from(hour) * HOUR_NS;
        let (request, risk) = evaluation(hour, 500_000);
        let outcome = run(
            &mut runtime,
            UnifiedCommand::EvaluateAndStage {
                command_id: UnifiedCommandId(bytes(30 + hour)),
                request: Box::new(request),
                risk: Box::new(risk),
                recorded_at_ns: now,
            },
        );
        assert_eq!(outcome.detail, UnifiedDetail::NoTrade);
        assert_eq!(runtime.snapshot().cash_available_micros, 3_000_000);
        assert_eq!(runtime.snapshot().cash_reserved_micros, 0);
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn eight_hour_split_merge_soak_conserves_capital_each_hour() {
    let mut runtime = UnifiedPairedTradingRuntime::new(config()).expect("runtime");
    fund_and_reconcile(&mut runtime, 1);
    let mut block = 2_u64;
    for hour in 2_u8..=9 {
        let now = i64::from(hour) * HOUR_NS;
        let base = 50 + (hour - 2) * 8;
        let up = accounting_ledger::TokenKey::new(format!("soak-{hour}"), format!("up-{hour}"))
            .expect("up");
        let down = accounting_ledger::TokenKey::new(format!("soak-{hour}"), format!("down-{hour}"))
            .expect("down");
        let split_id = ConversionId(bytes(100 + hour));
        run(
            &mut runtime,
            UnifiedCommand::RequestConversion {
                command_id: UnifiedCommandId(bytes(base)),
                conversion_id: split_id,
                request: ConversionRequest::Split {
                    up: up.clone(),
                    down: down.clone(),
                    quantity_micros: 500_000,
                },
                recorded_at_ns: now,
            },
        );
        for (offset, event) in [
            ConversionEvent::Pending {
                external_transaction_id: format!("split-{hour}"),
            },
            ConversionEvent::Confirmed {
                transaction_hash: format!("split-hash-{hour}"),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let step = u8::try_from(offset + 1).expect("step");
            run(
                &mut runtime,
                UnifiedCommand::ObserveConversion {
                    command_id: UnifiedCommandId(bytes(base + step)),
                    observation: ConversionObservation {
                        conversion_id: split_id,
                        source_sequence: u64::from(step),
                        event,
                        event_time_ns: now + i64::from(step),
                        received_time_ns: now + i64::from(step),
                    },
                    recorded_at_ns: now + i64::from(step),
                },
            );
        }
        let chain = exact_chain(&runtime, block, now + 3);
        run(
            &mut runtime,
            UnifiedCommand::Reconcile {
                command_id: UnifiedCommandId(bytes(base + 3)),
                chain,
                evaluated_at_ns: now + 3,
                recorded_at_ns: now + 3,
            },
        );
        block += 1;
        let merge_id = ConversionId(bytes(120 + hour));
        run(
            &mut runtime,
            UnifiedCommand::RequestConversion {
                command_id: UnifiedCommandId(bytes(base + 4)),
                conversion_id: merge_id,
                request: ConversionRequest::Merge {
                    lock_id: LockId(bytes(140 + hour)),
                    up,
                    down,
                    quantity_micros: 500_000,
                },
                recorded_at_ns: now + 4,
            },
        );
        for (offset, event) in [
            ConversionEvent::Pending {
                external_transaction_id: format!("merge-{hour}"),
            },
            ConversionEvent::Confirmed {
                transaction_hash: format!("merge-hash-{hour}"),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let step = u8::try_from(offset + 5).expect("step");
            run(
                &mut runtime,
                UnifiedCommand::ObserveConversion {
                    command_id: UnifiedCommandId(bytes(base + step)),
                    observation: ConversionObservation {
                        conversion_id: merge_id,
                        source_sequence: u64::from(step - 4),
                        event,
                        event_time_ns: now + i64::from(step),
                        received_time_ns: now + i64::from(step),
                    },
                    recorded_at_ns: now + i64::from(step),
                },
            );
        }
        let chain = exact_chain(&runtime, block, now + 7);
        run(
            &mut runtime,
            UnifiedCommand::Reconcile {
                command_id: UnifiedCommandId(bytes(base + 7)),
                chain,
                evaluated_at_ns: now + 7,
                recorded_at_ns: now + 7,
            },
        );
        block += 1;
        assert_eq!(runtime.snapshot().cash_available_micros, 3_000_000);
        assert_eq!(runtime.snapshot().cash_reserved_micros, 0);
    }
    assert_eq!(runtime.snapshot().confirmed_conversion_count, 16);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]
    #[test]
    fn exact_duplicate_funding_never_posts_twice(amount in 1_i128..10_000_000) {
        let mut runtime = UnifiedPairedTradingRuntime::new(config()).expect("runtime");
        let command = UnifiedCommand::Fund {
            command_id: UnifiedCommandId(bytes(1)),
            amount_micros: amount,
            recorded_at_ns: HOUR_NS,
        };
        let first = runtime.apply(&command).expect("first");
        let second = runtime.apply(&command).expect("duplicate");
        prop_assert_eq!(first, second);
        prop_assert_eq!(runtime.snapshot().cash_available_micros, amount);
        prop_assert_eq!(runtime.snapshot().accepted_commands, 1);
    }
}

#[test]
fn invalid_config_is_rejected() {
    assert!(matches!(
        UnifiedPairedTradingRuntime::new(ReconcilerConfig {
            chain_id: 0,
            wallet: String::new(),
            confirmation_grace_ns: -1,
            max_intents: 0,
            max_tokens: 0,
        }),
        Err(Error::Config)
    ));
}
