use super::*;
use common_types::{PriceMicros, QuantityMicros, QuotePriceMicros, ReferenceQuantityE8};
use complete_set_arbitrage::{
    ArbitrageCommandId, ArbitrageConstraints, ArbitrageDirection, ArbitrageEvaluationId,
    ArbitrageRequest,
};
use feed_supervisor::{SupervisorMode, SupervisorSnapshot};
use live_market_state::{ActorMode, ActorSnapshot};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use market_session::{
    CoordinationFrame, MarketSessionCoordinator, SessionKey, SessionSourceState, TokenBookView,
};
use portfolio_risk::{
    candidate_set_digest, order_exposure_digest, DecisionReason, GroupMultiplier,
};
use proptest::prelude::*;
use public_market_data::{Asset, MarketIdentity, BTC_HOURLY};
use reference_market_data::{
    CandleData, CandleInterval, InProgressCandle, ReferenceHealth, ReferenceSnapshot,
    ReferenceSymbol,
};
use std::collections::BTreeMap;
use strategy_proposal::capture_context;
use tempfile::tempdir;

const HOUR_MS: i64 = 3_600_000;
const ACTIVE_NS: i64 = HOUR_MS * 1_000_000;

fn bytes(value: u8) -> [u8; 32] {
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
    capture_context(&snapshot, &frame, &identity, ACTIVE_NS, ACTIVE_NS + 1_000).expect("context")
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

fn risk_frame(context: &strategy_proposal::StrategyContext, cash: i128) -> PairRiskFrame {
    PairRiskFrame {
        reconciliation: ReconciliationRiskGate {
            reconciliation_digest: bytes(40),
            ready: true,
            evaluated_at_ns: Some(ACTIVE_NS),
            ledger_digest: Some(bytes(41)),
            chain_block_number: Some(7),
        },
        ledger: LedgerRiskView {
            ledger_digest: bytes(41),
            halted: false,
            cash_available_micros: cash,
            cash_reserved_micros: 0,
            available_tokens: Vec::new(),
            reserved_tokens: Vec::new(),
            locked_tokens: Vec::new(),
        },
        markets: vec![BinaryMarketRisk {
            condition_id: context.condition_id.clone(),
            up: context.up_token.clone(),
            down: context.down_token.clone(),
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
    }
}

fn command(id: u8, context: strategy_proposal::StrategyContext, cash: i128) -> PairedCommand {
    let frame = risk_frame(&context, cash);
    PairedCommand::Evaluate {
        command_id: PairedCommandId(bytes(id)),
        arbitrage_command: Box::new(arbitrage(context)),
        risk_frame: Box::new(frame),
        recorded_at_ns: ACTIVE_NS,
    }
}

#[test]
fn authentic_pair_runs_detector_two_proposals_and_combined_fill_risk() {
    let source = context(400_000, 500_000);
    let decision = PairedRuntime::default()
        .apply(&command(1, source, 3_000_000))
        .expect("paired decision");
    assert_eq!(decision.status, PairedStatus::RiskEligible);
    assert_eq!(decision.proposal_decisions.len(), 2);
    assert!(decision.verify_digest());
    let risk = decision.risk_decision.expect("combined risk");
    assert_eq!(risk.status, RiskStatus::Approve);
    assert_eq!(risk.scenario_count, 18);
    let first = decision.proposal_decisions[0]
        .candidate
        .as_ref()
        .expect("first");
    let second = decision.proposal_decisions[1]
        .candidate
        .as_ref()
        .expect("second");
    assert_ne!(risk.candidate_order_digest, order_exposure_digest(first));
    assert_ne!(risk.candidate_order_digest, order_exposure_digest(second));
}

#[test]
fn combined_capacity_rejects_when_each_leg_would_fit_alone() {
    let source = context(400_000, 500_000);
    let decision = PairedRuntime::default()
        .apply(&command(1, source, 700_000))
        .expect("no trade");
    assert_eq!(decision.status, PairedStatus::NoTrade);
    let risk = decision.risk_decision.expect("risk");
    assert_eq!(risk.reason, DecisionReason::CandidateCapacity);
}

#[test]
fn detector_no_opportunity_never_reaches_proposal_or_risk() {
    let source = context(600_000, 500_000);
    let decision = PairedRuntime::default()
        .apply(&command(1, source, 3_000_000))
        .expect("no opportunity");
    assert_eq!(decision.reason, PairedReason::DetectorNoOpportunity);
    assert!(decision.proposal_decisions.is_empty());
    assert!(decision.risk_decision.is_none());
}

#[test]
fn candidate_set_digest_is_ordered_and_not_single_order_authority() {
    let source = context(400_000, 500_000);
    let value = command(1, source, 3_000_000);
    let decision = PairedRuntime::default().apply(&value).expect("decision");
    let risk = decision.risk_decision.expect("risk");
    let first = decision.proposal_decisions[0]
        .candidate
        .clone()
        .expect("first");
    let second = decision.proposal_decisions[1]
        .candidate
        .clone()
        .expect("second");
    let PairedCommand::Evaluate { risk_frame, .. } = value;
    let request = RiskRequest {
        reconciliation: risk_frame.reconciliation,
        ledger: risk_frame.ledger,
        markets: risk_frame.markets,
        open_orders: risk_frame.open_orders,
        candidate: first.clone(),
        additional_candidates: vec![second.clone()],
        shocks: risk_frame.shocks,
        limits: risk_frame.limits,
        evaluated_at_ns: risk_frame.evaluated_at_ns,
    };
    assert_eq!(risk.candidate_order_digest, candidate_set_digest(&request));
    assert_ne!(risk.candidate_order_digest, order_exposure_digest(&first));
    let mut reversed = request;
    reversed.candidate = second;
    reversed.additional_candidates = vec![first];
    assert_ne!(candidate_set_digest(&reversed), risk.candidate_order_digest);
}

#[test]
fn command_identity_conflict_halts_all_children_absorbingly() {
    let source = context(400_000, 500_000);
    let first = command(1, source.clone(), 3_000_000);
    let mut runtime = PairedRuntime::default();
    runtime.apply(&first).expect("first");
    assert_eq!(
        runtime.apply(&first).expect("duplicate"),
        runtime.snapshot().last_decision.expect("last")
    );
    assert_eq!(
        runtime.apply(&command(1, source, 2_000_000)),
        Err(Error::IdempotencyConflict)
    );
    assert!(runtime.is_halted());
}

#[test]
fn arbitrage_decision_cannot_be_rewrapped_under_a_new_pair_command() {
    let source = context(600_000, 500_000);
    let mut runtime = PairedRuntime::default();
    runtime
        .apply(&command(1, source.clone(), 3_000_000))
        .expect("first no opportunity");
    assert_eq!(
        runtime.apply(&command(2, source, 3_000_000)),
        Err(Error::Boundary)
    );
    assert!(runtime.is_halted());
}

#[test]
fn codec_replay_and_checkpoint_reproduce_composed_state() {
    let value = command(1, context(400_000, 500_000), 3_000_000);
    let encoded = encode_command(&value).expect("encode");
    assert_eq!(decode_command(&encoded).expect("decode"), value);
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
    let mut durable = DurablePairedRuntime::new(
        writer,
        PairedRecovery {
            runtime: PairedRuntime::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    durable.apply(&value).expect("apply");
    let digest = durable.runtime().snapshot().digest;
    let checkpoint = PairedCheckpoint {
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
fn sync_failure_never_installs_composed_child_state() {
    let mut durable = DurablePairedRuntime::new(
        FailingJournal::default(),
        PairedRecovery {
            runtime: PairedRuntime::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    let value = command(1, context(400_000, 500_000), 3_000_000);
    assert!(matches!(
        durable.apply(&value),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(durable.runtime().snapshot().accepted_commands, 0);
    assert!(matches!(
        durable.apply(&value),
        Err(StorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn reducing_available_cash_cannot_turn_pair_no_trade_into_eligible(cash in 0_i128..900_000_i128) {
        let source = context(400_000, 500_000);
        let decision = PairedRuntime::default()
            .apply(&command(1, source, cash))
            .expect("decision");
        prop_assert_eq!(decision.status, PairedStatus::NoTrade);
    }
}
