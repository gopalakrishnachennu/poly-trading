use super::*;
use common_types::{PriceMicros, QuantityMicros, QuotePriceMicros, ReferenceQuantityE8};
use feed_supervisor::{SupervisorMode, SupervisorSnapshot};
use live_market_state::{ActorMode, ActorSnapshot};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use market_session::{
    CoordinationFrame, MarketSessionCoordinator, SessionKey, SessionSourceState, TokenBookView,
};
use proptest::prelude::*;
use public_market_data::{Asset, MarketIdentity, BTC_HOURLY};
use reference_market_data::{
    CandleData, CandleInterval, InProgressCandle, ReferenceHealth, ReferenceSnapshot,
    ReferenceSymbol,
};
use std::collections::BTreeMap;
use strategy_proposal::{capture_context, ProposalEngine};
use tempfile::tempdir;

const HOUR_MS: i64 = 3_600_000;
const ACTIVE_NS: i64 = HOUR_MS * 1_000_000;

fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn identity() -> MarketIdentity {
    MarketIdentity {
        asset: Asset::Bitcoin,
        event_id: "event-a".to_owned(),
        market_id: "market-a".to_owned(),
        condition_id: format!("0x{}", "a".repeat(64)),
        question_id: format!("0x{}", "b".repeat(64)),
        event_slug: "event-a".to_owned(),
        market_slug: "market-a".to_owned(),
        series_id: BTC_HOURLY.id.to_owned(),
        series_slug: BTC_HOURLY.slug.to_owned(),
        title: "Up or Down".to_owned(),
        start_time_ms: HOUR_MS,
        end_time_ms: 2 * HOUR_MS,
        resolution_source: "https://www.binance.com/en/trade/BTC_USDT".to_owned(),
        description: "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the BTC/USDT 1 hour candle that begins on the time and date specified in the title. Otherwise, this market will resolve to \"Down\". The close and open displayed for the relevant candle will be used once the data for that candle is finalized. This market is about the Binance pair, not according to other exchanges or trading pairs.".to_owned(),
        up_token_id: "up-a".to_owned(),
        down_token_id: "down-a".to_owned(),
        rules_fingerprint: bytes(7),
    }
}

fn book(bid: i64, ask: i64, quantity: i64, authoritative: bool) -> TokenBookView {
    TokenBookView {
        authoritative,
        best_bid: Some((
            PriceMicros::new(bid).expect("bid"),
            QuantityMicros::new(quantity).expect("quantity"),
        )),
        best_ask: Some((
            PriceMicros::new(ask).expect("ask"),
            QuantityMicros::new(quantity).expect("quantity"),
        )),
    }
}

fn candle() -> CandleData {
    CandleData {
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
    }
}

fn context(
    ready: bool,
    up: TokenBookView,
    down: TokenBookView,
    valid_until_ns: i64,
) -> StrategyContext {
    let identity = identity();
    let market_digest = bytes(3);
    let reference_digest = bytes(4);
    let market = ActorSnapshot {
        mode: if ready {
            ActorMode::Ready
        } else {
            ActorMode::Stale
        },
        ready,
        epoch: 1,
        last_sequence: Some(10),
        book_count: 2,
        digest: market_digest,
        last_market_event_ns: Some(ACTIVE_NS),
        last_market_received_ns: Some(ACTIVE_NS),
        halt_reason: None,
    };
    let reference = ReferenceSnapshot {
        health: if ready {
            ReferenceHealth::Ready
        } else {
            ReferenceHealth::Disconnected
        },
        epoch: 2,
        last_sequence: Some(20),
        digest: reference_digest,
        last_reference_received_ns: Some(ACTIVE_NS),
        symbols: BTreeMap::new(),
    };
    let supervision = SupervisorSnapshot {
        mode: if ready {
            SupervisorMode::Ready
        } else {
            SupervisorMode::MarketStale
        },
        ready,
        evaluated_at_ns: Some(ACTIVE_NS),
        market_epoch: market.epoch,
        market_sequence: market.last_sequence,
        market_digest: bytes(5),
        market_state_digest: market_digest,
        reference_epoch: reference.epoch,
        reference_sequence: reference.last_sequence,
        reference_digest: bytes(6),
        reference_state_digest: reference_digest,
        halt_reason: None,
        digest: bytes(8),
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
                up_book: Some(up),
                down_book: Some(down),
                in_progress: Some(InProgressCandle(candle())),
                finalized: None,
            },
        )]
        .into_iter()
        .collect(),
    };
    let mut coordinator = MarketSessionCoordinator::default();
    coordinator.register(identity.clone()).expect("register");
    let snapshot = coordinator.evaluate(&frame).expect("coordinate");
    capture_context(&snapshot, &frame, &identity, ACTIVE_NS, valid_until_ns).expect("context")
}

fn buy_context() -> StrategyContext {
    context(
        true,
        book(390_000, 400_000, 2_000_000, true),
        book(490_000, 500_000, 1_500_000, true),
        ACTIVE_NS + 1_000,
    )
}

fn sell_context() -> StrategyContext {
    context(
        true,
        book(600_000, 610_000, 2_000_000, true),
        book(550_000, 560_000, 1_500_000, true),
        ACTIVE_NS + 1_000,
    )
}

fn constraints() -> ArbitrageConstraints {
    ArbitrageConstraints {
        min_quantity_micros: 100_000,
        max_quantity_micros: 1_000_000,
        partial_fill_micros: 50_000,
        up_max_fee_micros: 4_000,
        down_max_fee_micros: 6_000,
        conversion_max_cost_micros: 1_000,
        min_net_profit_micros: 1,
        min_roi_bps: 1,
    }
}

fn command(
    command: u8,
    evaluation: u8,
    direction: ArbitrageDirection,
    context: StrategyContext,
    constraints: ArbitrageConstraints,
    at: i64,
) -> ArbitrageCommand {
    ArbitrageCommand::Evaluate {
        command_id: ArbitrageCommandId(bytes(command)),
        request: ArbitrageRequest {
            evaluation_id: ArbitrageEvaluationId(bytes(evaluation)),
            direction,
            context: Box::new(context),
            constraints,
            evaluated_at_ns: at,
            expires_at_ns: at + 100,
        },
        recorded_at_ns: at,
    }
}

#[test]
fn buy_pair_rounds_cost_up_and_emits_exactly_two_inert_intents() {
    let source = buy_context();
    let decision = ArbitrageEngine::default()
        .apply(&command(
            1,
            1,
            ArbitrageDirection::BuyPair,
            source.clone(),
            constraints(),
            ACTIVE_NS,
        ))
        .expect("opportunity");
    assert_eq!(decision.status, ArbitrageStatus::Opportunity);
    assert!(decision.verify_digest());
    let plan = decision.plan.expect("plan");
    assert!(plan.verify_digest());
    assert_eq!(plan.quantity_micros, 1_000_000);
    assert_eq!(plan.up_value_micros, 400_000);
    assert_eq!(plan.down_value_micros, 500_000);
    assert_eq!(plan.deployed_capital_micros, 911_000);
    assert_eq!(plan.net_profit_micros, 89_000);
    assert_eq!(plan.roi_bps, 976);
    assert_eq!(plan.intents[0].token, source.up_token);
    assert_eq!(plan.intents[1].token, source.down_token);
    assert_ne!(plan.intents[0].proposal_id, plan.intents[1].proposal_id);
    let mut proposal = ProposalEngine::default();
    for (index, intent) in plan.intents.into_iter().enumerate() {
        let result = proposal
            .apply(&strategy_proposal::ProposalCommand::Evaluate {
                command_id: strategy_proposal::ProposalCommandId(bytes(
                    u8::try_from(index).expect("two intents") + 20,
                )),
                context: Box::new(source.clone()),
                recorded_at_ns: intent.evaluated_at_ns,
                intent: Box::new(intent),
            })
            .expect("inert candidate");
        assert!(result.candidate.is_some());
    }
}

#[test]
fn sell_pair_rounds_proceeds_down_and_accounts_for_split_cost() {
    let mut value = constraints();
    value.conversion_max_cost_micros = 2_000;
    let decision = ArbitrageEngine::default()
        .apply(&command(
            1,
            1,
            ArbitrageDirection::SellPair,
            sell_context(),
            value,
            ACTIVE_NS,
        ))
        .expect("opportunity");
    let plan = decision.plan.expect("plan");
    assert_eq!(plan.up_value_micros, 600_000);
    assert_eq!(plan.down_value_micros, 550_000);
    assert_eq!(plan.deployed_capital_micros, 1_002_000);
    assert_eq!(plan.net_profit_micros, 138_000);
    assert_eq!(plan.roi_bps, 1_377);
    assert!(plan
        .intents
        .iter()
        .all(|intent| intent.side == OrderSide::Sell));
}

#[test]
fn exact_profit_and_roi_thresholds_are_inclusive() {
    let source = buy_context();
    let mut exact = constraints();
    exact.min_net_profit_micros = 89_000;
    exact.min_roi_bps = 976;
    assert_eq!(
        ArbitrageEngine::default()
            .apply(&command(
                1,
                1,
                ArbitrageDirection::BuyPair,
                source.clone(),
                exact.clone(),
                ACTIVE_NS,
            ))
            .expect("inclusive")
            .status,
        ArbitrageStatus::Opportunity
    );
    exact.min_net_profit_micros += 1;
    assert_eq!(
        ArbitrageEngine::default()
            .apply(&command(
                2,
                2,
                ArbitrageDirection::BuyPair,
                source.clone(),
                exact,
                ACTIVE_NS,
            ))
            .expect("profit reject")
            .reason,
        ArbitrageReason::ProfitBelowMinimum
    );
    let mut roi = constraints();
    roi.min_roi_bps = 977;
    assert_eq!(
        ArbitrageEngine::default()
            .apply(&command(
                3,
                3,
                ArbitrageDirection::BuyPair,
                source,
                roi,
                ACTIVE_NS,
            ))
            .expect("roi reject")
            .reason,
        ArbitrageReason::RoiBelowMinimum
    );
}

#[test]
fn degraded_expired_invalid_and_illiquid_inputs_are_attributable() {
    let degraded = context(
        false,
        book(390_000, 400_000, 2_000_000, true),
        book(490_000, 500_000, 1_500_000, true),
        ACTIVE_NS + 100,
    );
    assert_eq!(
        ArbitrageEngine::default()
            .apply(&command(
                1,
                1,
                ArbitrageDirection::BuyPair,
                degraded,
                constraints(),
                ACTIVE_NS,
            ))
            .expect("degraded")
            .reason,
        ArbitrageReason::SourceNotReady
    );
    let expired = context(
        true,
        book(390_000, 400_000, 2_000_000, true),
        book(490_000, 500_000, 1_500_000, true),
        ACTIVE_NS,
    );
    assert_eq!(
        ArbitrageEngine::default()
            .apply(&command(
                2,
                2,
                ArbitrageDirection::BuyPair,
                expired,
                constraints(),
                ACTIVE_NS + 1,
            ))
            .expect("expired")
            .reason,
        ArbitrageReason::ContextExpired
    );
    let mut request_expired = command(
        8,
        8,
        ArbitrageDirection::BuyPair,
        buy_context(),
        constraints(),
        ACTIVE_NS,
    );
    let ArbitrageCommand::Evaluate { request, .. } = &mut request_expired;
    request.expires_at_ns = ACTIVE_NS - 1;
    assert_eq!(
        ArbitrageEngine::default()
            .apply(&request_expired)
            .expect("request expired")
            .reason,
        ArbitrageReason::RequestExpired
    );
    let mut invalid = constraints();
    invalid.min_quantity_micros = 0;
    assert_eq!(
        ArbitrageEngine::default()
            .apply(&command(
                3,
                3,
                ArbitrageDirection::BuyPair,
                buy_context(),
                invalid,
                ACTIVE_NS,
            ))
            .expect("invalid")
            .reason,
        ArbitrageReason::InvalidConstraints
    );
    let mut illiquid = constraints();
    illiquid.min_quantity_micros = 1_600_000;
    illiquid.max_quantity_micros = 2_000_000;
    assert_eq!(
        ArbitrageEngine::default()
            .apply(&command(
                4,
                4,
                ArbitrageDirection::BuyPair,
                buy_context(),
                illiquid,
                ACTIVE_NS,
            ))
            .expect("illiquid")
            .reason,
        ArbitrageReason::InsufficientLiquidity
    );
}

#[test]
fn rounding_never_creates_a_false_micro_profit() {
    let source = context(
        true,
        book(249_998, 249_999, 2, true),
        book(249_999, 250_000, 2, true),
        ACTIVE_NS + 100,
    );
    let mut value = constraints();
    value.min_quantity_micros = 2;
    value.max_quantity_micros = 2;
    value.partial_fill_micros = 1;
    value.up_max_fee_micros = 0;
    value.down_max_fee_micros = 0;
    value.conversion_max_cost_micros = 0;
    assert_eq!(
        ArbitrageEngine::default()
            .apply(&command(
                1,
                1,
                ArbitrageDirection::BuyPair,
                source,
                value,
                ACTIVE_NS,
            ))
            .expect("rounded")
            .reason,
        ArbitrageReason::NonPositiveProfit
    );
}

#[test]
fn idempotency_evaluation_reuse_and_context_equivocation_are_strict() {
    let source = buy_context();
    let original = command(
        1,
        1,
        ArbitrageDirection::BuyPair,
        source.clone(),
        constraints(),
        ACTIVE_NS,
    );
    let mut engine = ArbitrageEngine::default();
    let first = engine.apply(&original).expect("first");
    assert_eq!(engine.apply(&original).expect("duplicate"), first);
    assert_eq!(
        engine
            .apply(&command(
                2,
                1,
                ArbitrageDirection::SellPair,
                source.clone(),
                constraints(),
                ACTIVE_NS,
            ))
            .expect("reuse")
            .reason,
        ArbitrageReason::EvaluationAlreadyUsed
    );
    assert_eq!(
        engine.apply(&command(
            1,
            2,
            ArbitrageDirection::BuyPair,
            source,
            constraints(),
            ACTIVE_NS,
        )),
        Err(Error::IdempotencyConflict)
    );
    assert!(engine.is_halted());

    let first_context = buy_context();
    let changed = context(
        true,
        book(390_000, 400_000, 2_000_000, true),
        book(490_000, 500_000, 1_500_000, true),
        ACTIVE_NS + 999,
    );
    let mut history = ArbitrageEngine::default();
    history
        .apply(&command(
            3,
            3,
            ArbitrageDirection::BuyPair,
            first_context,
            constraints(),
            ACTIVE_NS,
        ))
        .expect("history first");
    assert_eq!(
        history.apply(&command(
            4,
            4,
            ArbitrageDirection::BuyPair,
            changed,
            constraints(),
            ACTIVE_NS,
        )),
        Err(Error::ContextHistory)
    );
}

#[test]
fn codec_replay_and_checkpoint_are_deterministic() {
    let value = command(
        1,
        1,
        ArbitrageDirection::BuyPair,
        buy_context(),
        constraints(),
        ACTIVE_NS,
    );
    let encoded = encode_command(&value).expect("encode");
    assert_eq!(decode_command(&encoded).expect("decode"), value);
    let mut trailing = encoded;
    trailing.push(b'x');
    assert!(matches!(decode_command(&trailing), Err(Error::Json(_))));

    let directory = tempdir().expect("directory");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 64 * 1024,
            max_segment_records: 1,
        },
    )
    .expect("writer");
    let mut durable = DurableArbitrageEngine::new(
        writer,
        ArbitrageRecovery {
            engine: ArbitrageEngine::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    durable.apply(&value).expect("apply");
    let digest = durable.engine().snapshot().digest;
    let checkpoint = ArbitrageCheckpoint {
        sequence: 0,
        engine_digest: digest,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("write");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, Some(checkpoint)).expect("recover");
    assert_eq!(recovered.engine.snapshot().digest, digest);
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
fn sync_failure_never_installs_an_opportunity_and_poisons_owner() {
    let mut durable = DurableArbitrageEngine::new(
        FailingJournal::default(),
        ArbitrageRecovery {
            engine: ArbitrageEngine::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    let value = command(
        1,
        1,
        ArbitrageDirection::BuyPair,
        buy_context(),
        constraints(),
        ACTIVE_NS,
    );
    assert!(matches!(
        durable.apply(&value),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(durable.engine().snapshot().accepted_commands, 0);
    assert!(matches!(
        durable.apply(&value),
        Err(StorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn increasing_fee_budget_cannot_increase_profit(extra in 0_i128..100_000_i128) {
        let source = buy_context();
        let base = constraints();
        let base_plan = ArbitrageEngine::default()
            .apply(&command(1, 1, ArbitrageDirection::BuyPair, source.clone(), base.clone(), ACTIVE_NS))
            .expect("base")
            .plan
            .expect("base plan");
        let mut higher = base;
        higher.up_max_fee_micros += extra;
        let changed = ArbitrageEngine::default()
            .apply(&command(2, 2, ArbitrageDirection::BuyPair, source, higher, ACTIVE_NS))
            .expect("changed");
        if let Some(plan) = changed.plan {
            prop_assert!(plan.net_profit_micros <= base_plan.net_profit_micros);
            prop_assert!(plan.roi_bps <= base_plan.roi_bps);
        } else {
            prop_assert_ne!(changed.status, ArbitrageStatus::Opportunity);
        }
    }
}
