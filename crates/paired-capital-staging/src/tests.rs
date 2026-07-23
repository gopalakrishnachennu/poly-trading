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
use paired_opportunity_runtime::{PairRiskFrame, PairedCommandId};
use portfolio_risk::{BinaryMarketRisk, GroupMultiplier, RiskLimits, ShockProfile};
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

fn identity() -> MarketIdentity {
    MarketIdentity {
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
    }
}

fn book(bid: i64, ask: i64) -> TokenBookView {
    TokenBookView {
        authoritative: true,
        best_bid: Some((
            PriceMicros::new(bid).expect("bid"),
            QuantityMicros::new(2_000_000).expect("quantity"),
        )),
        best_ask: Some((
            PriceMicros::new(ask).expect("ask"),
            QuantityMicros::new(2_000_000).expect("quantity"),
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

fn context(up_ask: i64, down_ask: i64) -> strategy_proposal::StrategyContext {
    let identity = identity();
    let market_digest = bytes(3);
    let reference_digest = bytes(4);
    let market = ActorSnapshot {
        mode: ActorMode::Ready,
        ready: true,
        epoch: 1,
        last_sequence: Some(10),
        book_count: 2,
        digest: market_digest,
        last_market_event_ns: Some(ACTIVE_NS),
        last_market_received_ns: Some(ACTIVE_NS),
        halt_reason: None,
    };
    let reference = ReferenceSnapshot {
        health: ReferenceHealth::Ready,
        epoch: 2,
        last_sequence: Some(20),
        digest: reference_digest,
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
        market_state_digest: market_digest,
        reference_epoch: 2,
        reference_sequence: Some(20),
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
                up_book: Some(book(up_ask - 10_000, up_ask)),
                down_book: Some(book(down_ask - 10_000, down_ask)),
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
    strategy_proposal::capture_context(&snapshot, &frame, &identity, ACTIVE_NS, ACTIVE_NS + 1_000)
        .expect("context")
}

fn arbitrage(context: strategy_proposal::StrategyContext) -> ArbitrageCommand {
    ArbitrageCommand::Evaluate {
        command_id: ArbitrageCommandId(bytes(31)),
        request: ArbitrageRequest {
            evaluation_id: ArbitrageEvaluationId(bytes(32)),
            direction: ArbitrageDirection::BuyPair,
            context: Box::new(context),
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
    }
}

fn paired(runtime: &CapitalStagingRuntime, id: u8, up: i64, down: i64) -> PairedCommand {
    let context = context(up, down);
    let ledger = runtime.ledger_risk_view();
    PairedCommand::Evaluate {
        command_id: PairedCommandId(bytes(id)),
        arbitrage_command: Box::new(arbitrage(context.clone())),
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

fn fund(runtime: &mut CapitalStagingRuntime, amount: i128) {
    runtime
        .apply(&StagingCommand::Fund {
            command_id: StagingCommandId(bytes(1)),
            amount_micros: amount,
            recorded_at_ns: ACTIVE_NS - 1,
        })
        .expect("fund");
}

fn stage(runtime: &mut CapitalStagingRuntime, id: u8) -> StagingDecision {
    let command = paired(runtime, id, 400_000, 500_000);
    runtime
        .apply(&StagingCommand::Stage {
            command_id: StagingCommandId(bytes(id + 10)),
            paired_command: Box::new(command),
            recorded_at_ns: ACTIVE_NS,
        })
        .expect("stage")
}

#[test]
fn reserves_two_exact_buy_legs_or_neither() {
    let mut runtime = CapitalStagingRuntime::default();
    fund(&mut runtime, 3_000_000);
    let decision = stage(&mut runtime, 2);
    assert!(decision.verify_digest());
    let StagingDetail::FullyReserved { record } = decision.detail else {
        panic!("expected stage");
    };
    assert!(record.verify_digest());
    assert_eq!(runtime.snapshot().active_stage_count, 1);
    assert_eq!(runtime.ledger_risk_view().cash_available_micros, 2_090_000);
    assert_eq!(runtime.ledger_risk_view().cash_reserved_micros, 910_000);
    let amounts: Vec<_> = record
        .reservation_ids
        .iter()
        .map(|id| {
            runtime
                .reservation(*id)
                .expect("reservation")
                .original_micros
        })
        .collect();
    assert_eq!(amounts, vec![404_000, 506_000]);
}

#[test]
fn detector_no_trade_never_reserves_capital() {
    let mut runtime = CapitalStagingRuntime::default();
    fund(&mut runtime, 3_000_000);
    let before = runtime.ledger_risk_view();
    let command = paired(&runtime, 2, 600_000, 500_000);
    let decision = runtime
        .apply(&StagingCommand::Stage {
            command_id: StagingCommandId(bytes(12)),
            paired_command: Box::new(command),
            recorded_at_ns: ACTIVE_NS,
        })
        .expect("no trade");
    assert!(matches!(decision.detail, StagingDetail::PairNoTrade { .. }));
    assert_eq!(runtime.ledger_risk_view(), before);
}

#[test]
fn substituted_ledger_halts_without_reservation() {
    let mut runtime = CapitalStagingRuntime::default();
    fund(&mut runtime, 3_000_000);
    let before = runtime.ledger_risk_view();
    let mut command = paired(&runtime, 2, 400_000, 500_000);
    let PairedCommand::Evaluate { risk_frame, .. } = &mut command;
    risk_frame.ledger.cash_available_micros += 1;
    assert_eq!(
        runtime.apply(&StagingCommand::Stage {
            command_id: StagingCommandId(bytes(12)),
            paired_command: Box::new(command),
            recorded_at_ns: ACTIVE_NS,
        }),
        Err(Error::Boundary)
    );
    assert_eq!(runtime.ledger_risk_view(), before);
    assert!(runtime.is_halted());
}

#[test]
fn injected_second_leg_failure_rolls_back_first_leg() {
    let mut runtime = CapitalStagingRuntime::default();
    fund(&mut runtime, 3_000_000);
    runtime
        .apply(&StagingCommand::InjectFault {
            command_id: StagingCommandId(bytes(2)),
            fault: StagingFault::BeforeSecondReservation,
            recorded_at_ns: ACTIVE_NS - 1,
        })
        .expect("arm");
    let before = runtime.ledger_risk_view();
    let command = paired(&runtime, 3, 400_000, 500_000);
    assert_eq!(
        runtime.apply(&StagingCommand::Stage {
            command_id: StagingCommandId(bytes(13)),
            paired_command: Box::new(command),
            recorded_at_ns: ACTIVE_NS,
        }),
        Err(Error::Boundary)
    );
    assert_eq!(runtime.ledger_risk_view(), before);
    assert_eq!(runtime.snapshot().active_stage_count, 0);
}

#[test]
fn abort_releases_both_and_duplicate_abort_halts_without_drift() {
    let mut runtime = CapitalStagingRuntime::default();
    fund(&mut runtime, 3_000_000);
    let staged = stage(&mut runtime, 2);
    let StagingDetail::FullyReserved { record } = staged.detail else {
        panic!("stage");
    };
    let aborted = runtime
        .apply(&StagingCommand::Abort {
            command_id: StagingCommandId(bytes(20)),
            stage_id: record.stage_id,
            recorded_at_ns: ACTIVE_NS + 1,
        })
        .expect("abort");
    assert!(matches!(aborted.detail, StagingDetail::Aborted { .. }));
    assert_eq!(runtime.ledger_risk_view().cash_available_micros, 3_000_000);
    assert_eq!(runtime.ledger_risk_view().cash_reserved_micros, 0);
    let before = runtime.ledger_risk_view();
    assert_eq!(
        runtime.apply(&StagingCommand::Abort {
            command_id: StagingCommandId(bytes(21)),
            stage_id: record.stage_id,
            recorded_at_ns: ACTIVE_NS + 2,
        }),
        Err(Error::StageInactive)
    );
    assert_eq!(runtime.ledger_risk_view(), before);
}

#[test]
fn identity_conflict_is_absorbing() {
    let mut runtime = CapitalStagingRuntime::default();
    let first = StagingCommand::Fund {
        command_id: StagingCommandId(bytes(1)),
        amount_micros: 3_000_000,
        recorded_at_ns: ACTIVE_NS - 1,
    };
    runtime.apply(&first).expect("first");
    assert!(runtime.apply(&first).is_ok());
    assert_eq!(
        runtime.apply(&StagingCommand::Fund {
            command_id: StagingCommandId(bytes(1)),
            amount_micros: 2_000_000,
            recorded_at_ns: ACTIVE_NS - 1,
        }),
        Err(Error::IdempotencyConflict)
    );
    assert!(runtime.is_halted());
}

#[test]
fn durable_replay_and_checkpoint_reproduce_staged_state() {
    let directory = tempdir().expect("directory");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 256 * 1024,
            max_segment_records: 1,
        },
    )
    .expect("writer");
    let mut durable = DurableCapitalStaging::new(
        writer,
        StagingRecovery {
            runtime: CapitalStagingRuntime::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    durable
        .apply(&StagingCommand::Fund {
            command_id: StagingCommandId(bytes(1)),
            amount_micros: 3_000_000,
            recorded_at_ns: ACTIVE_NS - 1,
        })
        .expect("fund");
    let command = paired(durable.runtime(), 2, 400_000, 500_000);
    durable
        .apply(&StagingCommand::Stage {
            command_id: StagingCommandId(bytes(12)),
            paired_command: Box::new(command),
            recorded_at_ns: ACTIVE_NS,
        })
        .expect("stage");
    let digest = durable.runtime().snapshot().digest;
    let checkpoint = StagingCheckpoint {
        sequence: 1,
        runtime_digest: digest,
    };
    let checkpoint_path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&checkpoint_path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, Some(checkpoint)).expect("recover");
    assert_eq!(recovered.runtime.snapshot().digest, digest);
    assert_eq!(recovered.runtime.snapshot().active_stage_count, 1);
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
fn sync_failure_never_installs_funding() {
    let mut durable = DurableCapitalStaging::new(
        FailingJournal::default(),
        StagingRecovery {
            runtime: CapitalStagingRuntime::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    assert!(matches!(
        durable.apply(&StagingCommand::Fund {
            command_id: StagingCommandId(bytes(1)),
            amount_micros: 3_000_000,
            recorded_at_ns: ACTIVE_NS - 1,
        }),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(
        durable.runtime().ledger_risk_view().cash_available_micros,
        0
    );
}

#[test]
fn codec_rejects_trailing_data() {
    let command = StagingCommand::Fund {
        command_id: StagingCommandId(bytes(1)),
        amount_micros: 3_000_000,
        recorded_at_ns: ACTIVE_NS - 1,
    };
    let encoded = encode_command(&command).expect("encode");
    assert_eq!(decode_command(&encoded).expect("decode"), command);
    let mut trailing = encoded;
    trailing.push(b'x');
    assert!(matches!(decode_command(&trailing), Err(Error::Json(_))));
}

proptest! {
    #[test]
    fn insufficient_capital_never_creates_a_partial_stage(amount in 0_i128..1_506_000_i128) {
        let mut runtime = CapitalStagingRuntime::default();
        fund(&mut runtime, amount);
        let command = paired(&runtime, 2, 400_000, 500_000);
        let result = runtime.apply(&StagingCommand::Stage {
            command_id: StagingCommandId(bytes(12)),
            paired_command: Box::new(command),
            recorded_at_ns: ACTIVE_NS,
        });
        prop_assert!(result.is_ok());
        prop_assert_eq!(runtime.snapshot().active_stage_count, 0);
        prop_assert_eq!(runtime.ledger_risk_view().cash_reserved_micros, 0);
        prop_assert_eq!(runtime.ledger_risk_view().cash_available_micros, amount);
    }
}
