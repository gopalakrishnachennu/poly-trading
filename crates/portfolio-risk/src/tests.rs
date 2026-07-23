use super::*;
use accounting_ledger::{AccountingLedger, CommandId as LedgerCommandId, LedgerCommand};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use std::fs;
use tempfile::tempdir;

fn cid(value: u8) -> RiskCommandId {
    RiskCommandId([value; 32])
}

fn oid(value: u8) -> RiskOrderId {
    RiskOrderId([value; 32])
}

fn up(condition: &str) -> TokenKey {
    TokenKey::new(condition, format!("{condition}-up")).expect("token")
}

fn down(condition: &str) -> TokenKey {
    TokenKey::new(condition, format!("{condition}-down")).expect("token")
}

fn market(condition: &str, group: &str) -> BinaryMarketRisk {
    BinaryMarketRisk {
        condition_id: condition.to_owned(),
        up: up(condition),
        down: down(condition),
        shock_group: group.to_owned(),
    }
}

fn balance(token: TokenKey, amount: i128) -> ConfirmedTokenBalance {
    ConfirmedTokenBalance {
        token,
        balance_micros: amount,
    }
}

fn ledger() -> LedgerRiskView {
    LedgerRiskView {
        ledger_digest: [2; 32],
        halted: false,
        cash_available_micros: 1_000_000,
        cash_reserved_micros: 0,
        available_tokens: Vec::new(),
        reserved_tokens: Vec::new(),
        locked_tokens: Vec::new(),
    }
}

fn gate() -> ReconciliationRiskGate {
    ReconciliationRiskGate {
        reconciliation_digest: [1; 32],
        ready: true,
        evaluated_at_ns: Some(100),
        ledger_digest: Some([2; 32]),
        chain_block_number: Some(10),
    }
}

fn candidate() -> OrderExposure {
    OrderExposure {
        order_id: oid(9),
        token: up("btc"),
        side: OrderSide::Buy,
        quantity_micros: 100_000,
        partial_fill_micros: 50_000,
        limit_price_micros: 400_000,
        max_fee_micros: 0,
    }
}

fn limits() -> RiskLimits {
    RiskLimits {
        capital_floor_micros: 950_000,
        operational_reserve_micros: 0,
        pending_settlement_reserve_micros: 0,
        max_gross_exposure_micros: 10_000_000,
        max_condition_exposure_micros: 10_000_000,
        max_group_exposure_micros: 10_000_000,
        reserved_cash_haircut_bps: 10_000,
        available_token_haircut_bps: 10_000,
        reserved_token_haircut_bps: 10_000,
        locked_token_haircut_bps: 10_000,
        max_reconciliation_age_ns: 100,
        max_open_orders: 8,
        max_scenarios: 100_000,
    }
}

fn request() -> RiskRequest {
    RiskRequest {
        reconciliation: gate(),
        ledger: ledger(),
        markets: vec![market("btc", "crypto")],
        open_orders: Vec::new(),
        candidate: candidate(),
        additional_candidates: Vec::new(),
        shocks: vec![ShockProfile {
            shock_id: "baseline".to_owned(),
            group_multipliers: Vec::new(),
        }],
        limits: limits(),
        evaluated_at_ns: 110,
    }
}

fn command(id: u8, request: RiskRequest) -> RiskCommand {
    let at = request.evaluated_at_ns;
    RiskCommand::Evaluate {
        command_id: cid(id),
        request,
        recorded_at_ns: at,
    }
}

fn evaluate_request(request: RiskRequest) -> RiskDecision {
    PortfolioRiskEngine::default()
        .apply(&command(1, request))
        .expect("decision")
}

#[test]
fn approves_only_after_all_fill_outcome_and_shock_scenarios_pass() {
    let decision = evaluate_request(request());
    assert_eq!(decision.status, DecisionStatus::Approve);
    assert_eq!(decision.reason, DecisionReason::AllLimitsSatisfied);
    assert_eq!(decision.scenario_count, 6);
    assert_eq!(decision.minimum_terminal_wealth_micros, Some(960_000));
    let witness = decision.witness.expect("witness");
    assert_eq!(witness.fill_quantities_micros, vec![100_000]);
    assert_eq!(witness.outcome_bits, vec![1]);
}

#[test]
fn paired_candidates_share_capacity_and_expand_fill_scenarios() {
    let mut value = request();
    value.additional_candidates = vec![OrderExposure {
        order_id: oid(10),
        token: down("btc"),
        side: OrderSide::Buy,
        quantity_micros: 100_000,
        partial_fill_micros: 50_000,
        limit_price_micros: 400_000,
        max_fee_micros: 0,
    }];
    let decision = evaluate_request(value.clone());
    assert_eq!(decision.status, DecisionStatus::Approve);
    assert_eq!(decision.scenario_count, 18);
    assert_eq!(
        decision.candidate_order_digest,
        candidate_set_digest(&value)
    );
    assert_ne!(
        decision.candidate_order_digest,
        order_exposure_digest(&value.candidate)
    );

    value.ledger.cash_available_micros = 70_000;
    value.limits.capital_floor_micros = 0;
    assert_eq!(
        evaluate_request(value).reason,
        DecisionReason::CandidateCapacity
    );
}

#[test]
fn capital_floor_is_inclusive_and_one_micro_above_returns_no_trade() {
    let mut boundary = request();
    boundary.limits.capital_floor_micros = 960_000;
    assert_eq!(evaluate_request(boundary).status, DecisionStatus::Approve);

    let mut rejected = request();
    rejected.limits.capital_floor_micros = 960_001;
    let decision = evaluate_request(rejected);
    assert_eq!(decision.status, DecisionStatus::NoTrade);
    assert_eq!(decision.reason, DecisionReason::CapitalFloor);
}

#[test]
fn reconciliation_readiness_freshness_and_digest_are_non_bypassable() {
    let mut unavailable = request();
    unavailable.reconciliation.ready = false;
    assert_eq!(
        evaluate_request(unavailable).reason,
        DecisionReason::ReconciliationNotReady
    );

    let mut stale = request();
    stale.evaluated_at_ns = 201;
    assert_eq!(
        evaluate_request(stale).reason,
        DecisionReason::ReconciliationStale
    );

    let mut provenance = request();
    provenance.ledger.ledger_digest = [8; 32];
    assert_eq!(
        evaluate_request(provenance).reason,
        DecisionReason::ProvenanceMismatch
    );

    let mut halted = request();
    halted.ledger.halted = true;
    assert_eq!(
        evaluate_request(halted).reason,
        DecisionReason::LedgerHalted
    );
}

#[test]
fn open_orders_must_exactly_back_reserved_cash_and_tokens() {
    let mut backed = request();
    backed.ledger.cash_available_micros = 959_000;
    backed.ledger.cash_reserved_micros = 41_000;
    backed.open_orders.push(OrderExposure {
        order_id: oid(1),
        token: up("btc"),
        side: OrderSide::Buy,
        quantity_micros: 100_000,
        partial_fill_micros: 50_000,
        limit_price_micros: 400_000,
        max_fee_micros: 1_000,
    });
    backed.limits.capital_floor_micros = 900_000;
    let decision = evaluate_request(backed.clone());
    assert_ne!(decision.reason, DecisionReason::ReservationMismatch);
    assert_eq!(decision.scenario_count, 18);

    backed.ledger.cash_reserved_micros -= 1;
    assert_eq!(
        evaluate_request(backed).reason,
        DecisionReason::ReservationMismatch
    );

    let mut sell = request();
    sell.ledger.available_tokens = vec![balance(up("btc"), 200_000)];
    sell.ledger.reserved_tokens = vec![balance(down("btc"), 100_000)];
    sell.open_orders.push(OrderExposure {
        order_id: oid(1),
        token: down("btc"),
        side: OrderSide::Sell,
        quantity_micros: 100_000,
        partial_fill_micros: 50_000,
        limit_price_micros: 600_000,
        max_fee_micros: 0,
    });
    assert_ne!(
        evaluate_request(sell.clone()).reason,
        DecisionReason::ReservationMismatch
    );
    sell.ledger.reserved_tokens[0].balance_micros += 1;
    assert_eq!(
        evaluate_request(sell).reason,
        DecisionReason::ReservationMismatch
    );
}

#[test]
fn partial_fill_fee_and_price_rounding_are_conservative() {
    assert_eq!(partial_fee(1, 1, 3), Ok(1));
    assert_eq!(buy_cost(333_333, 1), Ok(1));
    assert_eq!(sale_proceeds(333_333, 1), Ok(0));
}

#[test]
fn candidate_uses_only_available_assets() {
    let mut buy = request();
    buy.ledger.cash_available_micros = 39_999;
    buy.ledger.cash_reserved_micros = 0;
    assert_eq!(
        evaluate_request(buy).reason,
        DecisionReason::CandidateCapacity
    );

    let mut sell = request();
    sell.candidate.side = OrderSide::Sell;
    sell.candidate.quantity_micros = 100_000;
    sell.candidate.partial_fill_micros = 50_000;
    sell.ledger.reserved_tokens = vec![balance(up("btc"), 100_000)];
    sell.open_orders.push(OrderExposure {
        order_id: oid(1),
        token: up("btc"),
        side: OrderSide::Sell,
        quantity_micros: 100_000,
        partial_fill_micros: 0,
        limit_price_micros: 500_000,
        max_fee_micros: 0,
    });
    assert_eq!(
        evaluate_request(sell).reason,
        DecisionReason::CandidateCapacity
    );
}

#[test]
fn each_exposure_limit_has_an_exact_no_trade_reason() {
    let mut gross = request();
    gross.limits.capital_floor_micros = 0;
    gross.limits.max_gross_exposure_micros = 99_999;
    assert_eq!(
        evaluate_request(gross).reason,
        DecisionReason::GrossExposure
    );

    let mut condition = request();
    condition.limits.capital_floor_micros = 0;
    condition.limits.max_condition_exposure_micros = 99_999;
    assert_eq!(
        evaluate_request(condition).reason,
        DecisionReason::ConditionExposure
    );

    let mut group = request();
    group.limits.capital_floor_micros = 0;
    group.limits.max_group_exposure_micros = 99_999;
    assert_eq!(
        evaluate_request(group).reason,
        DecisionReason::GroupExposure
    );
}

#[test]
fn all_market_outcomes_and_correlated_shocks_are_counted() {
    let mut value = request();
    value.markets.push(market("eth", "crypto"));
    value.shocks.push(ShockProfile {
        shock_id: "crypto-crisis".to_owned(),
        group_multipliers: vec![GroupMultiplier {
            shock_group: "crypto".to_owned(),
            multiplier_bps: 5_000,
        }],
    });
    let decision = evaluate_request(value);
    assert_eq!(decision.scenario_count, 24);
}

#[test]
fn haircuts_and_operational_reserves_reduce_worst_case_wealth() {
    let mut base = request();
    base.ledger.cash_available_micros = 0;
    base.ledger.locked_tokens = vec![
        balance(down("btc"), 1_000_000),
        balance(up("btc"), 1_000_000),
    ];
    base.candidate.quantity_micros = 1;
    base.candidate.partial_fill_micros = 0;
    base.candidate.limit_price_micros = 0;
    base.limits.capital_floor_micros = 0;
    let full = evaluate_request(base.clone())
        .minimum_terminal_wealth_micros
        .expect("wealth");
    base.limits.locked_token_haircut_bps = 9_000;
    base.limits.operational_reserve_micros = 10_000;
    let reduced = evaluate_request(base)
        .minimum_terminal_wealth_micros
        .expect("wealth");
    assert_eq!(full, 1_000_000);
    assert_eq!(reduced, 890_000);
}

#[test]
fn every_asset_accessibility_category_has_an_independent_haircut() {
    let mut value = request();
    value.limits.capital_floor_micros = 0;
    value.candidate.quantity_micros = 1;
    value.candidate.partial_fill_micros = 0;
    value.candidate.limit_price_micros = 0;
    let token = up("btc");
    let one_token = BTreeMap::from([(token, 100_000)]);
    let empty = BTreeMap::new();
    let shock = &value.shocks[0];
    let outcome = [0];

    let cases = [
        (
            Holdings {
                available_cash: 0,
                reserved_cash: 100_000,
                available: empty.clone(),
                reserved: empty.clone(),
                locked: empty.clone(),
            },
            0,
        ),
        (
            Holdings {
                available_cash: 0,
                reserved_cash: 0,
                available: one_token.clone(),
                reserved: empty.clone(),
                locked: empty.clone(),
            },
            1,
        ),
        (
            Holdings {
                available_cash: 0,
                reserved_cash: 0,
                available: empty.clone(),
                reserved: one_token.clone(),
                locked: empty.clone(),
            },
            2,
        ),
        (
            Holdings {
                available_cash: 0,
                reserved_cash: 0,
                available: empty.clone(),
                reserved: empty,
                locked: one_token,
            },
            3,
        ),
    ];
    for (holdings, haircut_index) in cases {
        let full = terminal_wealth(&holdings, &value, &outcome, shock).expect("full haircut");
        let mut reduced = value.clone();
        match haircut_index {
            0 => reduced.limits.reserved_cash_haircut_bps = 9_000,
            1 => reduced.limits.available_token_haircut_bps = 9_000,
            2 => reduced.limits.reserved_token_haircut_bps = 9_000,
            3 => reduced.limits.locked_token_haircut_bps = 9_000,
            _ => unreachable!(),
        }
        assert_eq!(full, 100_000);
        assert_eq!(
            terminal_wealth(&holdings, &reduced, &outcome, shock),
            Ok(90_000)
        );
    }
}

#[test]
fn scenario_budget_fails_before_enumeration() {
    let mut value = request();
    value.limits.max_scenarios = 5;
    let decision = evaluate_request(value);
    assert_eq!(decision.status, DecisionStatus::NoTrade);
    assert_eq!(decision.reason, DecisionReason::ScenarioBudget);
    assert_eq!(decision.scenario_count, 0);
}

#[test]
fn idempotency_and_history_equivocation_halt() {
    let mut engine = PortfolioRiskEngine::default();
    let original = command(1, request());
    let first = engine.apply(&original).expect("decision");
    assert_eq!(engine.apply(&original), Ok(first));
    let mut conflict_request = request();
    conflict_request.limits.capital_floor_micros = 1;
    assert_eq!(
        engine.apply(&command(1, conflict_request)),
        Err(Error::IdempotencyConflict)
    );
    assert!(engine.is_halted());

    let mut history = PortfolioRiskEngine::default();
    history.apply(&command(1, request())).expect("first");
    let mut equivocal = request();
    equivocal.reconciliation.reconciliation_digest = [9; 32];
    assert_eq!(
        history.apply(&command(2, equivocal)),
        Err(Error::ReconciliationHistory)
    );

    let mut regression = PortfolioRiskEngine::default();
    regression.apply(&command(1, request())).expect("first");
    let mut older = request();
    older.evaluated_at_ns = 120;
    older.reconciliation.evaluated_at_ns = Some(99);
    assert_eq!(
        regression.apply(&command(2, older)),
        Err(Error::ReconciliationHistory)
    );
}

#[test]
fn codec_replay_and_checkpoint_are_deterministic() {
    let encoded = encode_command(&command(1, request())).expect("encode");
    assert_eq!(decode_command(&encoded), Ok(command(1, request())));
    let mut trailing = encoded;
    trailing.extend_from_slice(b" {}");
    assert!(matches!(decode_command(&trailing), Err(Error::Json(_))));

    let directory = tempdir().expect("directory");
    let writer = SegmentedJournalWriter::open(
        directory.path(),
        SegmentConfig {
            max_segment_bytes: 1_024 * 1_024,
            max_segment_records: 1,
        },
    )
    .expect("writer");
    let recovery = RiskRecovery {
        engine: PortfolioRiskEngine::default(),
        last_sequence: None,
    };
    let mut durable = DurableRiskEngine::new(writer, recovery).expect("durable");
    durable.apply(&command(1, request())).expect("first");
    let checkpoint = RiskCheckpoint {
        sequence: 0,
        risk_digest: durable.engine().snapshot().digest,
    };
    let mut second = request();
    second.evaluated_at_ns = 120;
    second.reconciliation.evaluated_at_ns = Some(110);
    durable.apply(&command(2, second)).expect("second");
    let online = durable.engine().snapshot().digest;
    drop(durable);
    let recovered = recover_segmented(directory.path(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.last_sequence, Some(1));
    assert_eq!(recovered.engine.snapshot().digest, online);
}

#[test]
fn checkpoint_file_and_sync_failure_are_fail_closed() {
    let directory = tempdir().expect("directory");
    let path = directory.path().join("risk.checkpoint");
    let checkpoint = RiskCheckpoint {
        sequence: 3,
        risk_digest: [4; 32],
    };
    write_checkpoint_create_new(&path, checkpoint).expect("write");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    assert!(write_checkpoint_create_new(&path, checkpoint).is_err());
    let corrupt = directory.path().join("corrupt.checkpoint");
    let mut bytes = fs::read(&path).expect("checkpoint bytes");
    bytes[24] ^= 1;
    fs::write(&corrupt, bytes).expect("corrupt checkpoint");
    assert!(matches!(
        read_checkpoint(corrupt),
        Err(StorageError::CheckpointChecksum)
    ));

    let journal = FailingJournal {
        last: None,
        fail_sync: true,
    };
    let mut durable = DurableRiskEngine::new(
        journal,
        RiskRecovery {
            engine: PortfolioRiskEngine::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    assert!(matches!(
        durable.apply(&command(1, request())),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(durable.engine().snapshot().accepted_commands, 0);
    assert!(matches!(
        durable.apply(&command(2, request())),
        Err(StorageError::Halted(_))
    ));
}

#[test]
fn durable_integrity_halt_is_recovered_as_an_absorbing_state() {
    let directory = tempdir().expect("directory");
    let writer = SegmentedJournalWriter::open(
        directory.path(),
        SegmentConfig {
            max_segment_bytes: 1_024 * 1_024,
            max_segment_records: 10,
        },
    )
    .expect("writer");
    let mut durable = DurableRiskEngine::new(
        writer,
        RiskRecovery {
            engine: PortfolioRiskEngine::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    durable.apply(&command(1, request())).expect("first");
    let mut equivocal = request();
    equivocal.reconciliation.reconciliation_digest = [9; 32];
    assert!(matches!(
        durable.apply(&command(2, equivocal)),
        Err(StorageError::Risk(Error::ReconciliationHistory))
    ));
    assert!(durable.engine().is_halted());
    drop(durable);

    let recovered = recover_segmented(directory.path(), None).expect("recover halt");
    assert_eq!(recovered.last_sequence, Some(1));
    assert!(recovered.engine.is_halted());
}

#[derive(Debug, Default)]
struct FailingJournal {
    last: Option<u64>,
    fail_sync: bool,
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
        if self.fail_sync {
            Err(JournalBackendError::Single(
                market_recorder::JournalError::Io(std::io::Error::other("injected sync")),
            ))
        } else {
            Ok(())
        }
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}

#[test]
fn actual_phase_two_ledger_risk_view_is_digest_bound() {
    let mut accounting = AccountingLedger::default();
    accounting
        .apply(&LedgerCommand::FundCollateral {
            command_id: LedgerCommandId([1; 32]),
            amount_micros: 1_000_000,
            recorded_at_ns: 1,
        })
        .expect("fund");
    let view = accounting.risk_view();
    assert_eq!(view.ledger_digest, accounting.snapshot().digest);
    assert_eq!(view.cash_available_micros, 1_000_000);
}

proptest! {
    #[test]
    fn worse_locked_haircut_never_increases_minimum_wealth(
        better in 1_u16..=10_000,
        worse in 0_u16..=10_000,
    ) {
        prop_assume!(worse <= better);
        let mut value = request();
        value.ledger.cash_available_micros = 0;
        value.ledger.locked_tokens = vec![
            balance(down("btc"), 1_000_000),
            balance(up("btc"), 1_000_000),
        ];
        value.candidate.quantity_micros = 1;
        value.candidate.partial_fill_micros = 0;
        value.candidate.limit_price_micros = 0;
        value.limits.capital_floor_micros = 0;
        value.limits.locked_token_haircut_bps = better;
        let better_wealth = evaluate_request(value.clone()).minimum_terminal_wealth_micros.expect("wealth");
        value.limits.locked_token_haircut_bps = worse;
        let worse_wealth = evaluate_request(value).minimum_terminal_wealth_micros.expect("wealth");
        prop_assert!(worse_wealth <= better_wealth);
    }

    #[test]
    fn adding_an_adverse_correlated_shock_never_increases_minimum_wealth(
        multiplier in 0_u16..=10_000,
    ) {
        let mut value = request();
        value.ledger.cash_available_micros = 0;
        value.ledger.locked_tokens = vec![
            balance(down("btc"), 1_000_000),
            balance(up("btc"), 1_000_000),
        ];
        value.candidate.quantity_micros = 1;
        value.candidate.partial_fill_micros = 0;
        value.candidate.limit_price_micros = 0;
        value.limits.capital_floor_micros = 0;
        let baseline = evaluate_request(value.clone()).minimum_terminal_wealth_micros.expect("wealth");
        value.shocks.push(ShockProfile {
            shock_id: "additional-adverse".to_owned(),
            group_multipliers: vec![GroupMultiplier {
                shock_group: "crypto".to_owned(),
                multiplier_bps: multiplier,
            }],
        });
        let stressed = evaluate_request(value).minimum_terminal_wealth_micros.expect("wealth");
        prop_assert!(stressed <= baseline);
    }
}
