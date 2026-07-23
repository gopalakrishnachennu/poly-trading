use super::*;
use common_types::{PriceMicros, QuantityMicros, QuotePriceMicros, ReferenceQuantityE8};
use feed_supervisor::{SupervisorMode, SupervisorSnapshot};
use live_market_state::{ActorMode, ActorSnapshot};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use market_session::{MarketSessionCoordinator, SessionSourceState};
use proptest::prelude::*;
use public_market_data::BTC_HOURLY;
use reference_market_data::{
    CandleData, CandleInterval, InProgressCandle, ReferenceHealth, ReferenceSnapshot,
    ReferenceSymbol,
};
use std::collections::BTreeMap;
use tempfile::tempdir;

const HOUR_MS: i64 = 3_600_000;
const ACTIVE_NS: i64 = HOUR_MS * 1_000_000;

fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn description() -> String {
    "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the BTC/USDT 1 hour candle that begins on the time and date specified in the title. Otherwise, this market will resolve to \"Down\". The close and open displayed for the relevant candle will be used once the data for that candle is finalized. This market is about the Binance pair, not according to other exchanges or trading pairs.".to_owned()
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
        description: description(),
        up_token_id: "up-a".to_owned(),
        down_token_id: "down-a".to_owned(),
        rules_fingerprint: bytes(7),
    }
}

fn book(authoritative: bool) -> TokenBookView {
    TokenBookView {
        authoritative,
        best_bid: Some((
            PriceMicros::new(400_000).expect("price"),
            QuantityMicros::new(2_000_000).expect("quantity"),
        )),
        best_ask: Some((
            PriceMicros::new(410_000).expect("price"),
            QuantityMicros::new(3_000_000).expect("quantity"),
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

fn frame(ready: bool) -> CoordinationFrame {
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
    let market_identity = identity();
    CoordinationFrame {
        now_ns: ACTIVE_NS,
        market,
        reference,
        supervision,
        sessions: [(
            SessionKey::from(&market_identity),
            SessionSourceState {
                up_book: Some(book(true)),
                down_book: Some(book(true)),
                in_progress: Some(InProgressCandle(candle())),
                finalized: None,
            },
        )]
        .into_iter()
        .collect(),
    }
}

fn context(ready: bool, captured_at_ns: i64, valid_until_ns: i64) -> StrategyContext {
    let identity = identity();
    let frame = frame(ready);
    let mut coordinator = MarketSessionCoordinator::default();
    coordinator.register(identity.clone()).expect("register");
    let snapshot = coordinator.evaluate(&frame).expect("coordinate");
    capture_context(&snapshot, &frame, &identity, captured_at_ns, valid_until_ns).expect("capture")
}

fn intent(proposal: u8, token: TokenKey, at: i64) -> ProposalIntent {
    ProposalIntent {
        proposal_id: ProposalId(bytes(proposal)),
        strategy: StrategyClass::CompleteSetArbitrage,
        token,
        side: OrderSide::Buy,
        quantity_micros: 100_000,
        partial_fill_micros: 50_000,
        limit_price_micros: 400_000,
        max_fee_micros: 1_000,
        evaluated_at_ns: at,
        expires_at_ns: at + 100,
    }
}

fn command(id: u8, context: StrategyContext, intent: ProposalIntent) -> ProposalCommand {
    let at = intent.evaluated_at_ns;
    ProposalCommand::Evaluate {
        command_id: ProposalCommandId(bytes(id)),
        context: Box::new(context),
        intent: Box::new(intent),
        recorded_at_ns: at,
    }
}

#[test]
fn exact_applied_session_frame_produces_only_an_inert_candidate() {
    let context = context(true, ACTIVE_NS, ACTIVE_NS + 100);
    assert!(context.current);
    assert!(context.active_ready);
    assert!(context.verify_digest());
    let proposal = intent(1, context.up_token.clone(), ACTIVE_NS);
    let decision = ProposalEngine::default()
        .apply(&command(1, context.clone(), proposal.clone()))
        .expect("candidate");
    assert_eq!(decision.status, ProposalStatus::Candidate);
    assert_eq!(decision.reason, ProposalReason::ContextAccepted);
    assert!(decision.verify_digest());
    let candidate = decision.candidate.expect("inert candidate");
    assert_eq!(candidate.token, context.up_token);
    assert_eq!(candidate.quantity_micros, proposal.quantity_micros);
    assert_ne!(candidate.order_id.0, proposal.proposal_id.0);
}

#[test]
fn substituted_or_unapplied_coordination_frame_cannot_create_context() {
    let identity = identity();
    let applied = frame(true);
    let mut coordinator = MarketSessionCoordinator::default();
    coordinator.register(identity.clone()).expect("register");
    let snapshot = coordinator.evaluate(&applied).expect("coordinate");
    let mut substituted = applied;
    substituted.market.digest = bytes(99);
    assert_eq!(
        capture_context(
            &snapshot,
            &substituted,
            &identity,
            ACTIVE_NS,
            ACTIVE_NS + 100
        ),
        Err(CaptureError::Provenance)
    );
    assert_eq!(
        capture_context(
            &snapshot,
            &substituted,
            &identity,
            ACTIVE_NS + 1,
            ACTIVE_NS + 100
        ),
        Err(CaptureError::Time)
    );
    assert_eq!(
        capture_context(
            &snapshot,
            &frame(true),
            &identity,
            ACTIVE_NS,
            ACTIVE_NS + MAX_CONTEXT_VALIDITY_NS + 1
        ),
        Err(CaptureError::Time)
    );
}

#[test]
fn degraded_missing_book_and_expiry_boundaries_are_attributable_rejections() {
    let degraded = context(false, ACTIVE_NS, ACTIVE_NS + 100);
    let degraded_intent = intent(1, degraded.up_token.clone(), ACTIVE_NS);
    assert_eq!(
        ProposalEngine::default()
            .apply(&command(1, degraded, degraded_intent))
            .expect("reject")
            .reason,
        ProposalReason::SourceNotReady
    );

    let exact = context(true, ACTIVE_NS, ACTIVE_NS);
    let exact_intent = intent(2, exact.up_token.clone(), ACTIVE_NS);
    assert_eq!(
        ProposalEngine::default()
            .apply(&command(2, exact, exact_intent))
            .expect("inclusive")
            .status,
        ProposalStatus::Candidate
    );
    let expired = context(true, ACTIVE_NS, ACTIVE_NS);
    let expired_intent = intent(3, expired.up_token.clone(), ACTIVE_NS + 1);
    assert_eq!(
        ProposalEngine::default()
            .apply(&command(3, expired, expired_intent))
            .expect("expired")
            .reason,
        ProposalReason::ContextExpired
    );
}

#[test]
fn token_economics_and_proposal_reuse_are_non_substitutable() {
    let source = context(true, ACTIVE_NS, ACTIVE_NS + 100);
    let unknown = TokenKey::new("other", "token").expect("token");
    assert_eq!(
        ProposalEngine::default()
            .apply(&command(1, source.clone(), intent(1, unknown, ACTIVE_NS)))
            .expect("unknown")
            .reason,
        ProposalReason::UnknownToken
    );

    let mut invalid = intent(2, source.up_token.clone(), ACTIVE_NS);
    invalid.partial_fill_micros = invalid.quantity_micros + 1;
    assert_eq!(
        ProposalEngine::default()
            .apply(&command(2, source.clone(), invalid))
            .expect("invalid")
            .reason,
        ProposalReason::InvalidEconomics
    );

    let mut engine = ProposalEngine::default();
    let first = intent(3, source.up_token.clone(), ACTIVE_NS);
    engine
        .apply(&command(3, source.clone(), first))
        .expect("first");
    let repeated = intent(3, source.up_token.clone(), ACTIVE_NS + 1);
    assert_eq!(
        engine
            .apply(&command(4, source, repeated))
            .expect("rejected replay")
            .reason,
        ProposalReason::ProposalAlreadyUsed
    );
}

#[test]
fn context_checksum_history_and_command_conflicts_halt_absorbingly() {
    let mut checksum = context(true, ACTIVE_NS, ACTIVE_NS + 100);
    checksum.valid_until_ns += 1;
    let checksum_intent = intent(1, checksum.up_token.clone(), ACTIVE_NS);
    let mut engine = ProposalEngine::default();
    assert_eq!(
        engine.apply(&command(1, checksum, checksum_intent)),
        Err(Error::ContextDigest)
    );
    assert!(engine.is_halted());

    let mut history = ProposalEngine::default();
    let first = context(true, ACTIVE_NS, ACTIVE_NS + 100);
    history
        .apply(&command(
            1,
            first.clone(),
            intent(1, first.up_token.clone(), ACTIVE_NS),
        ))
        .expect("first");
    let equivocated = context(true, ACTIVE_NS, ACTIVE_NS + 101);
    let equivocated_intent = intent(2, equivocated.up_token.clone(), ACTIVE_NS + 1);
    assert_eq!(
        history.apply(&command(2, equivocated, equivocated_intent)),
        Err(Error::ContextHistory)
    );

    let source = context(true, ACTIVE_NS, ACTIVE_NS + 100);
    let original = command(
        1,
        source.clone(),
        intent(1, source.up_token.clone(), ACTIVE_NS),
    );
    let mut conflict = ProposalEngine::default();
    conflict.apply(&original).expect("original");
    let changed = command(
        1,
        source.clone(),
        intent(2, source.up_token.clone(), ACTIVE_NS),
    );
    assert_eq!(conflict.apply(&changed), Err(Error::IdempotencyConflict));
}

#[test]
fn canonical_codec_round_trips_and_rejects_trailing_data() {
    let source = context(true, ACTIVE_NS, ACTIVE_NS + 100);
    let value = command(
        1,
        source.clone(),
        intent(1, source.up_token.clone(), ACTIVE_NS),
    );
    let encoded = encode_command(&value).expect("encode");
    assert_eq!(decode_command(&encoded).expect("decode"), value);
    let mut invalid = encoded;
    invalid.push(b'x');
    assert!(matches!(decode_command(&invalid), Err(Error::Json(_))));
}

#[test]
fn segmented_replay_and_checkpoint_reproduce_the_online_digest() {
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
    let recovery = ProposalRecovery {
        engine: ProposalEngine::default(),
        last_sequence: None,
    };
    let mut durable = DurableProposalEngine::new(writer, recovery).expect("durable");
    let source = context(true, ACTIVE_NS, ACTIVE_NS + 100);
    durable
        .apply(&command(
            1,
            source.clone(),
            intent(1, source.up_token.clone(), ACTIVE_NS),
        ))
        .expect("first");
    let expected = durable.engine().snapshot().digest;
    let checkpoint = ProposalCheckpoint {
        sequence: 0,
        engine_digest: expected,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("write");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, Some(checkpoint)).expect("recover");
    assert_eq!(recovered.engine.snapshot().digest, expected);
    assert_eq!(recovered.last_sequence, Some(0));
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
fn sync_failure_never_installs_a_candidate_and_poisons_the_owner() {
    let recovery = ProposalRecovery {
        engine: ProposalEngine::default(),
        last_sequence: None,
    };
    let mut durable =
        DurableProposalEngine::new(FailingJournal::default(), recovery).expect("durable");
    let source = context(true, ACTIVE_NS, ACTIVE_NS + 100);
    let value = command(
        1,
        source.clone(),
        intent(1, source.up_token.clone(), ACTIVE_NS),
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
    fn changing_economics_cannot_bypass_a_degraded_source(
        quantity in 1_i128..1_000_000_i128,
        price in 0_i64..=1_000_000_i64,
    ) {
        let source = context(false, ACTIVE_NS, ACTIVE_NS + 100);
        let mut proposed = intent(1, source.up_token.clone(), ACTIVE_NS);
        proposed.quantity_micros = quantity;
        proposed.partial_fill_micros = quantity;
        proposed.limit_price_micros = price;
        let decision = ProposalEngine::default()
            .apply(&command(1, source, proposed))
            .expect("attributable rejection");
        prop_assert_eq!(decision.reason, ProposalReason::SourceNotReady);
        prop_assert!(decision.candidate.is_none());
    }
}
