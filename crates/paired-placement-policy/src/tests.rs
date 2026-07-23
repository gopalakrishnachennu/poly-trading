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
                up_book: Some(book(up_ask - 10_000, up_ask)),
                down_book: Some(book(down_ask - 10_000, down_ask)),
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

fn paired(runtime: &PairedPlacementPolicy, id: u8) -> PairedCommand {
    let context = context(400_000, 500_000);
    let ledger = runtime.staging().ledger_risk_view();
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
        command_id: PairedCommandId(bytes(id)),
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

fn setup() -> (PairedPlacementPolicy, PairStageId) {
    let mut runtime = PairedPlacementPolicy::default();
    runtime
        .apply(&PairedPolicyCommand::Fund {
            command_id: PairedPolicyCommandId(bytes(1)),
            amount_micros: 3_000_000,
            recorded_at_ns: ACTIVE_NS - 2,
        })
        .expect("fund");
    let pair = paired(&runtime, 2);
    let decision = runtime
        .apply(&PairedPolicyCommand::Stage {
            command_id: PairedPolicyCommandId(bytes(2)),
            paired_command: Box::new(pair),
            recorded_at_ns: ACTIVE_NS,
        })
        .expect("stage");
    let stage_id = decision.stage_id.expect("stage id");
    runtime
        .apply(&PairedPolicyCommand::ObserveMode {
            command_id: PairedPolicyCommandId(bytes(3)),
            observation: ExchangeModeObservation {
                sequence: 1,
                mode: ExchangeMode::Normal,
                observed_at_ns: ACTIVE_NS,
                valid_until_ns: ACTIVE_NS + MAX_PERMISSION_NS,
            },
            recorded_at_ns: ACTIVE_NS,
        })
        .expect("mode");
    (runtime, stage_id)
}

fn authorize_first(runtime: &mut PairedPlacementPolicy, stage_id: PairStageId) -> PairPermit {
    runtime
        .apply(&PairedPolicyCommand::AuthorizeFirst {
            command_id: PairedPolicyCommandId(bytes(4)),
            stage_id,
            leg_index: 0,
            max_mode_age_ns: MAX_PERMISSION_NS,
            valid_until_ns: ACTIVE_NS + 50,
            recorded_at_ns: ACTIVE_NS,
        })
        .expect("authorize")
        .permit
        .expect("permit")
}

fn observe(
    runtime: &mut PairedPlacementPolicy,
    stage_id: PairStageId,
    permit: &PairPermit,
    id: u8,
    sequence: u64,
    state: LegState,
) -> PairedPolicyDecision {
    runtime
        .apply(&PairedPolicyCommand::ObserveLeg {
            command_id: PairedPolicyCommandId(bytes(id)),
            stage_id,
            leg_index: permit.leg_index,
            permit_id: permit.permit_id,
            state,
            source_sequence: sequence,
            observed_at_ns: ACTIVE_NS + i64::from(id),
            recorded_at_ns: ACTIVE_NS + i64::from(id),
        })
        .expect("observe")
}

#[test]
fn first_permission_binds_exact_stage_candidate_reservation_and_mode() {
    let (mut runtime, stage_id) = setup();
    let reserved = runtime.snapshot().reserved_cash_micros;
    let permit = authorize_first(&mut runtime, stage_id);
    assert!(permit.verify_digest());
    assert_eq!(permit.role, LegRole::First);
    assert_eq!(permit.stage_id, stage_id);
    assert_eq!(permit.mode_sequence, 1);
    assert_eq!(runtime.snapshot().reserved_cash_micros, reserved);
    assert_eq!(
        runtime.record(stage_id).expect("record").legs[0],
        LegState::Authorized
    );
}

#[test]
fn hedge_is_denied_until_first_is_fully_matched_then_exact_other_leg_is_permitted() {
    let (mut runtime, stage_id) = setup();
    let first = authorize_first(&mut runtime, stage_id);
    let early = runtime
        .apply(&PairedPolicyCommand::AuthorizeHedge {
            command_id: PairedPolicyCommandId(bytes(5)),
            stage_id,
            max_mode_age_ns: MAX_PERMISSION_NS,
            valid_until_ns: ACTIVE_NS + 60,
            recorded_at_ns: ACTIVE_NS,
        })
        .expect("deny");
    assert_eq!(early.reason, PairedPolicyReason::HedgeNotReady);
    observe(&mut runtime, stage_id, &first, 6, 1, LegState::Submitted);
    observe(&mut runtime, stage_id, &first, 7, 2, LegState::FullyMatched);
    let reserved_after_match = runtime.snapshot().reserved_cash_micros;
    assert_eq!(reserved_after_match, 910_000);
    let hedge = runtime
        .apply(&PairedPolicyCommand::AuthorizeHedge {
            command_id: PairedPolicyCommandId(bytes(8)),
            stage_id,
            max_mode_age_ns: MAX_PERMISSION_NS,
            valid_until_ns: ACTIVE_NS + 80,
            recorded_at_ns: ACTIVE_NS + 8,
        })
        .expect("hedge")
        .permit
        .expect("permit");
    assert_eq!(hedge.role, LegRole::Hedge);
    assert_eq!(hedge.leg_index, 1);
    assert_eq!(
        runtime.snapshot().reserved_cash_micros,
        reserved_after_match
    );
}

#[test]
fn permission_is_single_issue_and_no_fill_allows_paired_abort() {
    let (mut runtime, stage_id) = setup();
    let first = authorize_first(&mut runtime, stage_id);
    let duplicate = runtime
        .apply(&PairedPolicyCommand::AuthorizeFirst {
            command_id: PairedPolicyCommandId(bytes(5)),
            stage_id,
            leg_index: 0,
            max_mode_age_ns: MAX_PERMISSION_NS,
            valid_until_ns: ACTIVE_NS + 60,
            recorded_at_ns: ACTIVE_NS,
        })
        .expect("deny duplicate");
    assert_eq!(duplicate.reason, PairedPolicyReason::LegAlreadyAuthorized);
    observe(
        &mut runtime,
        stage_id,
        &first,
        6,
        1,
        LegState::NoFillTerminal,
    );
    let abort = runtime
        .apply(&PairedPolicyCommand::AbortSafe {
            command_id: PairedPolicyCommandId(bytes(7)),
            stage_id,
            recorded_at_ns: ACTIVE_NS + 7,
        })
        .expect("abort");
    assert_eq!(abort.reason, PairedPolicyReason::Aborted);
    assert_eq!(runtime.snapshot().reserved_cash_micros, 0);
}

#[test]
fn unknown_and_partial_states_retain_both_reservations_and_forbid_abort() {
    let (mut runtime, stage_id) = setup();
    let first = authorize_first(&mut runtime, stage_id);
    let reserved = runtime.snapshot().reserved_cash_micros;
    observe(&mut runtime, stage_id, &first, 6, 1, LegState::Submitted);
    observe(
        &mut runtime,
        stage_id,
        &first,
        7,
        2,
        LegState::PartiallyMatched,
    );
    observe(&mut runtime, stage_id, &first, 8, 3, LegState::Unknown);
    assert_eq!(runtime.snapshot().reserved_cash_micros, reserved);
    assert_eq!(runtime.snapshot().possible_exposure_legs, 1);
    let abort = runtime
        .apply(&PairedPolicyCommand::AbortSafe {
            command_id: PairedPolicyCommandId(bytes(9)),
            stage_id,
            recorded_at_ns: ACTIVE_NS + 9,
        })
        .expect("deny");
    assert_eq!(abort.reason, PairedPolicyReason::UnsafeAbort);
    assert_eq!(runtime.snapshot().reserved_cash_micros, reserved);
}

#[test]
fn expiry_does_not_release_then_safe_abort_releases_both() {
    let (mut runtime, stage_id) = setup();
    authorize_first(&mut runtime, stage_id);
    let reserved = runtime.snapshot().reserved_cash_micros;
    runtime
        .apply(&PairedPolicyCommand::Expire {
            command_id: PairedPolicyCommandId(bytes(5)),
            stage_id,
            recorded_at_ns: ACTIVE_NS + 50,
        })
        .expect("expire");
    assert_eq!(runtime.snapshot().reserved_cash_micros, reserved);
    assert_eq!(
        runtime.record(stage_id).expect("record").legs[0],
        LegState::Expired
    );
    let abort = runtime
        .apply(&PairedPolicyCommand::AbortSafe {
            command_id: PairedPolicyCommandId(bytes(6)),
            stage_id,
            recorded_at_ns: ACTIVE_NS + 51,
        })
        .expect("abort");
    assert_eq!(abort.reason, PairedPolicyReason::Aborted);
    assert_eq!(runtime.snapshot().reserved_cash_micros, 0);
}

#[test]
fn stale_mode_and_stage_window_deny_new_permissions() {
    let (mut runtime, stage_id) = setup();
    let stale = runtime
        .apply(&PairedPolicyCommand::AuthorizeFirst {
            command_id: PairedPolicyCommandId(bytes(4)),
            stage_id,
            leg_index: 0,
            max_mode_age_ns: 0,
            valid_until_ns: ACTIVE_NS + 50,
            recorded_at_ns: ACTIVE_NS + 1,
        })
        .expect("stale mode");
    assert_eq!(stale.reason, PairedPolicyReason::ModeStale);
    let stale_stage = runtime
        .apply(&PairedPolicyCommand::AuthorizeFirst {
            command_id: PairedPolicyCommandId(bytes(5)),
            stage_id,
            leg_index: 0,
            max_mode_age_ns: 2 * MAX_PERMISSION_NS,
            valid_until_ns: ACTIVE_NS + MAX_PERMISSION_NS + 2,
            recorded_at_ns: ACTIVE_NS + MAX_PERMISSION_NS + 1,
        })
        .expect("stale stage");
    assert_eq!(stale_stage.reason, PairedPolicyReason::StageStale);
}

#[test]
fn permit_subject_substitution_halts() {
    let (mut runtime, stage_id) = setup();
    let permit = authorize_first(&mut runtime, stage_id);
    assert_eq!(
        runtime.apply(&PairedPolicyCommand::ObserveLeg {
            command_id: PairedPolicyCommandId(bytes(5)),
            stage_id,
            leg_index: 0,
            permit_id: PairPermitId(bytes(99)),
            state: LegState::Submitted,
            source_sequence: 1,
            observed_at_ns: ACTIVE_NS + 1,
            recorded_at_ns: ACTIVE_NS + 1,
        }),
        Err(Error::Boundary)
    );
    assert!(runtime.is_halted());
    assert!(permit.verify_digest());
}

#[test]
fn exchange_mode_equivocation_halts_without_releasing_capital() {
    let (mut runtime, _) = setup();
    let reserved = runtime.snapshot().reserved_cash_micros;
    assert_eq!(
        runtime.apply(&PairedPolicyCommand::ObserveMode {
            command_id: PairedPolicyCommandId(bytes(4)),
            observation: ExchangeModeObservation {
                sequence: 1,
                mode: ExchangeMode::CancelOnly,
                observed_at_ns: ACTIVE_NS,
                valid_until_ns: ACTIVE_NS + MAX_PERMISSION_NS,
            },
            recorded_at_ns: ACTIVE_NS,
        }),
        Err(Error::ModeHistory)
    );
    assert!(runtime.is_halted());
    assert_eq!(runtime.snapshot().reserved_cash_micros, reserved);
}

#[test]
fn impossible_lifecycle_or_source_time_regression_is_absorbing() {
    let (mut runtime, stage_id) = setup();
    let first = authorize_first(&mut runtime, stage_id);
    observe(&mut runtime, stage_id, &first, 6, 1, LegState::Submitted);
    observe(
        &mut runtime,
        stage_id,
        &first,
        7,
        2,
        LegState::PartiallyMatched,
    );
    assert_eq!(
        runtime.apply(&PairedPolicyCommand::ObserveLeg {
            command_id: PairedPolicyCommandId(bytes(8)),
            stage_id,
            leg_index: 0,
            permit_id: first.permit_id,
            state: LegState::NoFillTerminal,
            source_sequence: 3,
            observed_at_ns: ACTIVE_NS + 8,
            recorded_at_ns: ACTIVE_NS + 8,
        }),
        Err(Error::Boundary)
    );
    assert!(runtime.is_halted());
    assert_eq!(runtime.snapshot().reserved_cash_micros, 910_000);
}

#[test]
fn durable_replay_checkpoint_and_sync_failure_are_fail_closed() {
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
    let mut durable = DurablePairedPolicy::new(
        writer,
        PairedPolicyRecovery {
            runtime: PairedPlacementPolicy::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    durable
        .apply(&PairedPolicyCommand::Fund {
            command_id: PairedPolicyCommandId(bytes(1)),
            amount_micros: 3_000_000,
            recorded_at_ns: ACTIVE_NS - 2,
        })
        .expect("fund");
    let pair = paired(durable.runtime(), 2);
    durable
        .apply(&PairedPolicyCommand::Stage {
            command_id: PairedPolicyCommandId(bytes(2)),
            paired_command: Box::new(pair),
            recorded_at_ns: ACTIVE_NS,
        })
        .expect("stage");
    let digest = durable.runtime().snapshot().digest;
    let checkpoint = PairedPolicyCheckpoint {
        sequence: 1,
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

    let mut failing = DurablePairedPolicy::new(
        FailingJournal::default(),
        PairedPolicyRecovery {
            runtime: PairedPlacementPolicy::default(),
            last_sequence: None,
        },
    )
    .expect("failing");
    assert!(matches!(
        failing.apply(&PairedPolicyCommand::Fund {
            command_id: PairedPolicyCommandId(bytes(1)),
            amount_micros: 3_000_000,
            recorded_at_ns: ACTIVE_NS - 2,
        }),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(failing.runtime().snapshot().reserved_cash_micros, 0);
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
    fn any_possible_fill_state_prevents_release(state in prop_oneof![
        Just(LegState::Submitted), Just(LegState::Delayed), Just(LegState::Live),
        Just(LegState::PartiallyMatched), Just(LegState::Unknown), Just(LegState::FullyMatched)
    ]) {
        prop_assert!(state.possible_fill());
        prop_assert!(!state.safe_terminal());
    }
}
