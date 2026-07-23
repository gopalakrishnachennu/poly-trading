use super::*;
use accounting_ledger::{CommandId as LedgerCommandId, ReservationId, TokenKey};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use order_intent_policy::{
    ExchangeMode, ExchangeModeObservation, PlacementRequest, PolicyCommand, PolicyCommandId,
    SignerPolicyFrame, TimeInForce,
};
use paper_execution::{
    ExchangeObservation, ExecutionCommand, ExecutionCommandId, MatchFill, RetryClass, UnknownReason,
};
use portfolio_risk::{BinaryMarketRisk, RiskCommandId, RiskLimits, RiskRequest, ShockProfile};
use proptest::prelude::*;
use settlement_reconciliation::{
    ChainTokenBalance, FinalizedChainSnapshot, ReconciliationCommand, TradeObservation, TradeStatus,
};
use std::collections::BTreeSet;
use tempfile::tempdir;

fn config() -> ReconcilerConfig {
    ReconcilerConfig {
        chain_id: 137,
        wallet: "paper-wallet".to_owned(),
        confirmation_grace_ns: 100,
        max_intents: 128,
        max_tokens: 16,
    }
}

fn bytes(value: u16) -> [u8; 32] {
    let mut result = [0_u8; 32];
    result[..2].copy_from_slice(&value.to_le_bytes());
    result
}

fn pipeline_id(value: u16) -> PipelineCommandId {
    PipelineCommandId(bytes(value))
}

fn order_id(value: u16) -> RiskOrderId {
    RiskOrderId(bytes(value))
}

fn token(name: &str) -> TokenKey {
    TokenKey::new("btc-hourly", name).expect("valid token")
}

fn wrap_accounting(id: u16, at: i64, command: LedgerCommand) -> PipelineCommand {
    PipelineCommand::Accounting {
        command_id: pipeline_id(id),
        command: Box::new(command),
        recorded_at_ns: at,
    }
}

fn wrap_reconciliation(id: u16, at: i64, command: ReconciliationCommand) -> PipelineCommand {
    PipelineCommand::Reconciliation {
        command_id: pipeline_id(id),
        command: Box::new(command),
        recorded_at_ns: at,
    }
}

fn wrap_risk(id: u16, at: i64, command: RiskCommand) -> PipelineCommand {
    PipelineCommand::Risk {
        command_id: pipeline_id(id),
        command: Box::new(command),
        recorded_at_ns: at,
    }
}

fn wrap_policy(id: u16, at: i64, command: PolicyCommand) -> PipelineCommand {
    PipelineCommand::Policy {
        command_id: pipeline_id(id),
        command: Box::new(command),
        recorded_at_ns: at,
    }
}

fn wrap_execution(id: u16, at: i64, command: ExecutionCommand) -> PipelineCommand {
    PipelineCommand::Execution {
        command_id: pipeline_id(id),
        command: Box::new(command),
        recorded_at_ns: at,
    }
}

fn chain(runtime: &PaperTradingRuntime, block: u64, at: i64) -> FinalizedChainSnapshot {
    let view = runtime.ledger().reconciliation_view(&BTreeSet::new());
    FinalizedChainSnapshot {
        chain_id: 137,
        wallet: "paper-wallet".to_owned(),
        block_number: block,
        block_hash: format!("paper-block-{block}"),
        finalized_at_ns: at - 1,
        observed_at_ns: at,
        collateral_micros: view.collateral_micros,
        token_balances: view
            .token_balances
            .into_iter()
            .map(|value| ChainTokenBalance {
                token: value.token,
                balance_micros: value.balance_micros,
            })
            .collect(),
    }
}

fn reconcile(id: u16, runtime: &PaperTradingRuntime, block: u64, at: i64) -> PipelineCommand {
    let frame = runtime
        .reconciler()
        .capture_frame(runtime.ledger(), chain(runtime, block, at), at);
    wrap_reconciliation(
        id,
        at,
        ReconciliationCommand::Reconcile {
            command_id: ReconciliationCommandId(bytes(id + 1_000)),
            frame,
            recorded_at_ns: at,
        },
    )
}

fn candidate(value: u16) -> OrderExposure {
    OrderExposure {
        order_id: order_id(value),
        token: token("up"),
        side: OrderSide::Buy,
        quantity_micros: 100_000,
        partial_fill_micros: 50_000,
        limit_price_micros: 400_000,
        max_fee_micros: 1_000,
    }
}

fn risk_request(runtime: &PaperTradingRuntime, order: OrderExposure, at: i64) -> RiskRequest {
    RiskRequest {
        reconciliation: runtime.reconciler().risk_gate(),
        ledger: runtime.ledger().risk_view(),
        markets: vec![BinaryMarketRisk {
            condition_id: "btc-hourly".to_owned(),
            up: token("up"),
            down: token("down"),
            shock_group: "crypto".to_owned(),
        }],
        open_orders: Vec::new(),
        candidate: order,
        additional_candidates: Vec::new(),
        shocks: vec![ShockProfile {
            shock_id: "baseline".to_owned(),
            group_multipliers: Vec::new(),
        }],
        limits: RiskLimits {
            capital_floor_micros: 900_000,
            operational_reserve_micros: 0,
            pending_settlement_reserve_micros: 0,
            max_gross_exposure_micros: 1_000_000,
            max_condition_exposure_micros: 1_000_000,
            max_group_exposure_micros: 1_000_000,
            reserved_cash_haircut_bps: 10_000,
            available_token_haircut_bps: 10_000,
            reserved_token_haircut_bps: 10_000,
            locked_token_haircut_bps: 10_000,
            max_reconciliation_age_ns: 1_000,
            max_open_orders: 8,
            max_scenarios: 1_000,
        },
        evaluated_at_ns: at,
    }
}

fn funded_ready() -> PaperTradingRuntime {
    let mut runtime = PaperTradingRuntime::new(config()).expect("runtime");
    runtime
        .apply(&wrap_accounting(
            1,
            1,
            LedgerCommand::FundCollateral {
                command_id: LedgerCommandId(bytes(1_001)),
                amount_micros: 1_000_000,
                recorded_at_ns: 1,
            },
        ))
        .expect("fund");
    let command = reconcile(2, &runtime, 1, 2);
    runtime.apply(&command).expect("initial reconcile");
    runtime
}

fn approve(runtime: &mut PaperTradingRuntime, value: u16, wrapper: u16, at: i64) -> RiskDecision {
    let order = candidate(value);
    let command = RiskCommand::Evaluate {
        command_id: RiskCommandId(bytes(wrapper + 1_000)),
        request: risk_request(runtime, order, at),
        recorded_at_ns: at,
    };
    let outcome = runtime
        .apply(&wrap_risk(wrapper, at, command))
        .expect("risk command");
    let PipelineDetail::Risk(decision) = outcome.detail else {
        panic!("risk decision expected")
    };
    assert_eq!(decision.status, RiskStatus::Approve);
    *decision
}

#[test]
fn multi_candidate_risk_decision_cannot_enter_single_order_paper_runtime() {
    let mut runtime = funded_ready();
    let mut request = risk_request(&runtime, candidate(90), 3);
    request.additional_candidates = vec![candidate(91)];
    let command = RiskCommand::Evaluate {
        command_id: RiskCommandId(bytes(1_090)),
        request,
        recorded_at_ns: 3,
    };
    assert!(matches!(
        runtime.apply(&wrap_risk(90, 3, command)),
        Err(Error::Boundary)
    ));
}

fn reserve(runtime: &mut PaperTradingRuntime, value: u16, wrapper: u16, at: i64) {
    runtime
        .apply(&wrap_accounting(
            wrapper,
            at,
            LedgerCommand::ReserveCollateral {
                command_id: LedgerCommandId(bytes(wrapper + 1_000)),
                reservation_id: ReservationId(bytes(value)),
                amount_micros: 41_000,
                recorded_at_ns: at,
            },
        ))
        .expect("reserve");
}

fn placement(order: OrderExposure, approval: RiskDecision, at: i64) -> PlacementRequest {
    PlacementRequest {
        approval,
        order,
        venue: "paper-polymarket".to_owned(),
        exchange_contract: "paper-ctf".to_owned(),
        post_only: true,
        marketable: false,
        time_in_force: TimeInForce::Gtc,
        signer_policy: SignerPolicyFrame {
            policy_id: bytes(900),
            venue: "paper-polymarket".to_owned(),
            exchange_contract: "paper-ctf".to_owned(),
            allowed_tokens: vec![token("up")],
            max_quantity_micros: 100_000,
            max_price_micros: 400_000,
            max_notional_micros: 41_000,
            allow_maker: true,
            allow_taker: false,
            valid_from_ns: 0,
            valid_until_ns: 1_000_000,
        },
        max_approval_age_ns: 1_000,
        max_mode_age_ns: 1_000,
        authorization_expires_at_ns: at + 500,
        evaluated_at_ns: at,
    }
}

fn authorize(
    runtime: &mut PaperTradingRuntime,
    value: u16,
    approval: RiskDecision,
    wrapper: u16,
    at: i64,
) -> (PlacementRequest, PolicyDecision) {
    runtime
        .apply(&wrap_policy(
            wrapper,
            at,
            PolicyCommand::ObserveMode {
                command_id: PolicyCommandId(bytes(wrapper + 1_000)),
                observation: ExchangeModeObservation {
                    sequence: u64::from(value),
                    mode: ExchangeMode::Normal,
                    observed_at_ns: at,
                    valid_until_ns: at + 1_000,
                },
                recorded_at_ns: at,
            },
        ))
        .expect("mode");
    let request = placement(candidate(value), approval, at + 1);
    let outcome = runtime
        .apply(&wrap_policy(
            wrapper + 1,
            at + 1,
            PolicyCommand::AuthorizePlacement {
                command_id: PolicyCommandId(bytes(wrapper + 1_001)),
                request: Box::new(request.clone()),
                recorded_at_ns: at + 1,
            },
        ))
        .expect("authorization");
    let PipelineDetail::Policy(decision) = outcome.detail else {
        panic!("policy decision expected")
    };
    assert_eq!(decision.status, order_intent_policy::PolicyStatus::Permit);
    (request, *decision)
}

fn submit(
    runtime: &mut PaperTradingRuntime,
    value: u16,
    request: PlacementRequest,
    permit: PolicyDecision,
    wrapper: u16,
    at: i64,
) {
    runtime
        .apply(&wrap_execution(
            wrapper,
            at,
            ExecutionCommand::Submit {
                command_id: ExecutionCommandId(bytes(wrapper + 1_000)),
                policy_decision: permit,
                placement: Box::new(request),
                local_submission_id: format!("paper-local-{value}"),
                recorded_at_ns: at,
            },
        ))
        .expect("submit");
}

fn observe(
    runtime: &mut PaperTradingRuntime,
    value: u16,
    wrapper: u16,
    sequence: u64,
    at: i64,
    event: ExchangeEvent,
) -> PipelineOutcome {
    runtime
        .apply(&wrap_execution(
            wrapper,
            at,
            ExecutionCommand::Observe {
                command_id: ExecutionCommandId(bytes(wrapper + 1_000)),
                observation: Box::new(ExchangeObservation {
                    order_id: order_id(value),
                    source_sequence: sequence,
                    exchange_order_id: Some(format!("paper-exchange-{value}")),
                    event,
                    event_time_ns: at,
                    received_time_ns: at,
                }),
                recorded_at_ns: at,
            },
        ))
        .expect("observation")
}

fn matched_runtime(register: bool) -> PaperTradingRuntime {
    let mut runtime = funded_ready();
    let approval = approve(&mut runtime, 10, 10, 10);
    reserve(&mut runtime, 10, 11, 11);
    let (request, permit) = authorize(&mut runtime, 10, approval, 12, 12);
    submit(&mut runtime, 10, request, permit, 14, 14);
    observe(&mut runtime, 10, 15, 0, 15, ExchangeEvent::Acknowledged);
    observe(
        &mut runtime,
        10,
        16,
        1,
        16,
        ExchangeEvent::Match {
            fill: MatchFill {
                fill_id: "fill-10".to_owned(),
                quantity_micros: 100_000,
                consideration_micros: 40_000,
                fee_micros: 1_000,
                cumulative_quantity_micros: 100_000,
                cumulative_consideration_micros: 40_000,
                cumulative_fee_micros: 1_000,
                ledger_command_id: LedgerCommandId(bytes(777)),
            },
            fully_matched: true,
        },
    );
    if register {
        runtime
            .apply(&PipelineCommand::RegisterHandoff {
                command_id: pipeline_id(17),
                order_id: order_id(10),
                handoff_index: 0,
                recorded_at_ns: 17,
            })
            .expect("handoff");
    }
    runtime
}

fn settle_all_handoffs(
    runtime: &mut PaperTradingRuntime,
    mut wrapper: u16,
    mut at: i64,
) -> (u16, i64) {
    let intents: Vec<_> = runtime
        .execution()
        .order(order_id(10))
        .expect("order")
        .handoffs
        .iter()
        .map(|handoff| handoff.intent.clone())
        .collect();
    for intent in intents {
        let matched_at = at;
        for status in [
            TradeStatus::Matched,
            TradeStatus::Mined,
            TradeStatus::Confirmed,
        ] {
            runtime
                .apply(&wrap_reconciliation(
                    wrapper,
                    at,
                    ReconciliationCommand::ObserveTrade {
                        command_id: ReconciliationCommandId(bytes(wrapper + 1_000)),
                        observation: TradeObservation {
                            trade_id: intent.trade_id.clone(),
                            order_id: intent.order_id.clone(),
                            token: intent.token.clone(),
                            side: intent.side,
                            quantity_micros: intent.quantity_micros,
                            consideration_micros: intent.consideration_micros,
                            fee_micros: intent.fee_micros,
                            status,
                            transaction_hash: matches!(
                                status,
                                TradeStatus::Mined | TradeStatus::Confirmed
                            )
                            .then(|| format!("paper-{}", intent.trade_id)),
                            matched_at_ns: matched_at,
                            updated_at_ns: at,
                        },
                        recorded_at_ns: at,
                    },
                ))
                .expect("settlement observation");
            at += 1;
            wrapper += 1;
        }
    }
    (wrapper, at)
}

#[test]
fn complete_fill_posts_once_and_reconciles_to_confirmed_chain_state() {
    let mut runtime = funded_ready();
    let approval = approve(&mut runtime, 10, 10, 10);
    reserve(&mut runtime, 10, 11, 11);
    let (request, permit) = authorize(&mut runtime, 10, approval, 12, 12);
    submit(&mut runtime, 10, request, permit, 14, 14);
    observe(&mut runtime, 10, 15, 0, 15, ExchangeEvent::Acknowledged);
    let fill_command_id = LedgerCommandId(bytes(777));
    let outcome = observe(
        &mut runtime,
        10,
        16,
        1,
        16,
        ExchangeEvent::Match {
            fill: MatchFill {
                fill_id: "fill-10".to_owned(),
                quantity_micros: 100_000,
                consideration_micros: 40_000,
                fee_micros: 1_000,
                cumulative_quantity_micros: 100_000,
                cumulative_consideration_micros: 40_000,
                cumulative_fee_micros: 1_000,
                ledger_command_id: fill_command_id,
            },
            fully_matched: true,
        },
    );
    assert!(matches!(outcome.detail, PipelineDetail::Execution(_)));
    runtime
        .apply(&PipelineCommand::RegisterHandoff {
            command_id: pipeline_id(17),
            order_id: order_id(10),
            handoff_index: 0,
            recorded_at_ns: 17,
        })
        .expect("handoff");
    runtime
        .apply(&wrap_accounting(
            18,
            18,
            LedgerCommand::ConfirmBuy {
                command_id: fill_command_id,
                reservation_id: ReservationId(bytes(10)),
                token: token("up"),
                quantity_micros: 100_000,
                consideration_micros: 40_000,
                fee_micros: 1_000,
                confirmation: "paper-tx-10".to_owned(),
                recorded_at_ns: 18,
            },
        ))
        .expect("ledger fill");

    settle_all_handoffs(&mut runtime, 19, 19);
    let command = reconcile(22, &runtime, 2, 22);
    runtime.apply(&command).expect("final reconcile");

    let snapshot = runtime.snapshot();
    assert!(!snapshot.halted);
    assert_eq!(snapshot.registered_handoff_count, 1);
    assert_eq!(snapshot.reserved_order_count, 0);
    assert!(runtime.reconciler().risk_gate().ready);
    assert_eq!(runtime.ledger().snapshot().cash_available_micros, 959_000);
    assert_eq!(
        runtime.ledger().risk_view().available_tokens[0].balance_micros,
        100_000
    );
}

#[test]
fn stale_cross_component_provenance_and_bad_reservations_fail_closed() {
    let mut provenance = funded_ready();
    let mut request = risk_request(&provenance, candidate(10), 10);
    request.ledger.cash_available_micros -= 1;
    let result = provenance.apply(&wrap_risk(
        10,
        10,
        RiskCommand::Evaluate {
            command_id: RiskCommandId(bytes(1_010)),
            request,
            recorded_at_ns: 10,
        },
    ));
    assert!(matches!(result, Err(Error::Boundary)));
    assert!(provenance.is_halted());

    let mut reservation = funded_ready();
    approve(&mut reservation, 10, 10, 10);
    let result = reservation.apply(&wrap_accounting(
        11,
        11,
        LedgerCommand::ReserveCollateral {
            command_id: LedgerCommandId(bytes(1_011)),
            reservation_id: ReservationId(bytes(10)),
            amount_micros: 40_999,
            recorded_at_ns: 11,
        },
    ));
    assert!(matches!(result, Err(Error::Reservation)));
    assert!(reservation.is_halted());
}

#[test]
fn placement_without_exact_capital_reservation_is_non_bypassable() {
    let mut runtime = funded_ready();
    let approval = approve(&mut runtime, 10, 10, 10);
    runtime
        .apply(&wrap_policy(
            11,
            11,
            PolicyCommand::ObserveMode {
                command_id: PolicyCommandId(bytes(1_011)),
                observation: ExchangeModeObservation {
                    sequence: 1,
                    mode: ExchangeMode::Normal,
                    observed_at_ns: 11,
                    valid_until_ns: 100,
                },
                recorded_at_ns: 11,
            },
        ))
        .expect("mode");
    let result = runtime.apply(&wrap_policy(
        12,
        12,
        PolicyCommand::AuthorizePlacement {
            command_id: PolicyCommandId(bytes(1_012)),
            request: Box::new(placement(candidate(10), approval, 12)),
            recorded_at_ns: 12,
        },
    ));
    assert!(matches!(result, Err(Error::Boundary)));
    assert!(runtime.is_halted());
}

#[test]
fn confirmed_posting_requires_the_exact_registered_execution_handoff() {
    let mut runtime = funded_ready();
    approve(&mut runtime, 10, 10, 10);
    reserve(&mut runtime, 10, 11, 11);
    let result = runtime.apply(&wrap_accounting(
        12,
        12,
        LedgerCommand::ConfirmBuy {
            command_id: LedgerCommandId(bytes(777)),
            reservation_id: ReservationId(bytes(10)),
            token: token("up"),
            quantity_micros: 100_000,
            consideration_micros: 40_000,
            fee_micros: 1_000,
            confirmation: "invented-confirmation".to_owned(),
            recorded_at_ns: 12,
        },
    ));
    assert!(matches!(result, Err(Error::Handoff)));
    assert!(runtime.is_halted());
    assert_eq!(runtime.ledger().snapshot().cash_reserved_micros, 41_000);
}

#[test]
fn active_or_unknown_execution_cannot_release_its_capital_reservation() {
    let mut runtime = funded_ready();
    let approval = approve(&mut runtime, 10, 10, 10);
    reserve(&mut runtime, 10, 11, 11);
    let (request, permit) = authorize(&mut runtime, 10, approval, 12, 12);
    submit(&mut runtime, 10, request, permit, 14, 14);
    let result = runtime.apply(&wrap_accounting(
        15,
        15,
        LedgerCommand::ReleaseReservation {
            command_id: LedgerCommandId(bytes(1_015)),
            reservation_id: ReservationId(bytes(10)),
            recorded_at_ns: 15,
        },
    ));
    assert!(matches!(result, Err(Error::Reservation)));
    assert!(runtime.is_halted());
    assert_eq!(runtime.ledger().snapshot().cash_reserved_micros, 41_000);
}

#[test]
fn downstream_decision_substitution_and_duplicate_handoff_halt_the_owner() {
    let mut decision = funded_ready();
    let approval = approve(&mut decision, 10, 10, 10);
    reserve(&mut decision, 10, 11, 11);
    let (request, mut permit) = authorize(&mut decision, 10, approval, 12, 12);
    permit.reason = order_intent_policy::PolicyReason::ModeAccepted;
    let result = decision.apply(&wrap_execution(
        14,
        14,
        ExecutionCommand::Submit {
            command_id: ExecutionCommandId(bytes(1_014)),
            policy_decision: permit,
            placement: Box::new(request),
            local_submission_id: "substituted".to_owned(),
            recorded_at_ns: 14,
        },
    ));
    assert!(matches!(result, Err(Error::Boundary)));
    assert!(decision.is_halted());

    let mut handoff = matched_runtime(true);
    let result = handoff.apply(&PipelineCommand::RegisterHandoff {
        command_id: pipeline_id(18),
        order_id: order_id(10),
        handoff_index: 0,
        recorded_at_ns: 18,
    });
    assert!(matches!(result, Err(Error::Handoff)));
    assert!(handoff.is_halted());
}

#[test]
fn caller_cannot_substitute_a_ledger_reconciliation_frame() {
    let mut runtime = funded_ready();
    let mut frame =
        runtime
            .reconciler()
            .capture_frame(runtime.ledger(), chain(&runtime, 2, 10), 10);
    frame.ledger.collateral_micros -= 1;
    let result = runtime.apply(&wrap_reconciliation(
        10,
        10,
        ReconciliationCommand::Reconcile {
            command_id: ReconciliationCommandId(bytes(1_010)),
            frame,
            recorded_at_ns: 10,
        },
    ));
    assert!(matches!(result, Err(Error::Boundary)));
    assert!(runtime.is_halted());
}

#[test]
fn attributable_no_trade_never_creates_an_approved_candidate() {
    let mut runtime = funded_ready();
    let mut request = risk_request(&runtime, candidate(10), 10);
    request.limits.capital_floor_micros = 1_000_000;
    let outcome = runtime
        .apply(&wrap_risk(
            10,
            10,
            RiskCommand::Evaluate {
                command_id: RiskCommandId(bytes(1_010)),
                request,
                recorded_at_ns: 10,
            },
        ))
        .expect("no trade");
    let PipelineDetail::Risk(decision) = outcome.detail else {
        panic!("risk decision expected")
    };
    assert_eq!(decision.status, RiskStatus::NoTrade);
    assert_eq!(runtime.snapshot().reserved_order_count, 0);
}

#[test]
fn deterministic_faults_are_one_shot_and_integrity_faults_are_absorbing() {
    let mut runtime = funded_ready();
    runtime
        .apply(&PipelineCommand::InjectFault {
            command_id: pipeline_id(10),
            fault: FaultPoint::BeforeRisk,
            recorded_at_ns: 10,
        })
        .expect("arm");
    let request = risk_request(&runtime, candidate(10), 11);
    let child = RiskCommand::Evaluate {
        command_id: RiskCommandId(bytes(1_011)),
        request,
        recorded_at_ns: 11,
    };
    let outcome = runtime
        .apply(&wrap_risk(11, 11, child.clone()))
        .expect("trigger");
    assert_eq!(
        outcome.detail,
        PipelineDetail::FaultTriggered(FaultPoint::BeforeRisk)
    );
    let outcome = runtime
        .apply(&wrap_risk(12, 11, child))
        .expect("retry under a new pipeline id");
    assert!(matches!(outcome.detail, PipelineDetail::Risk(_)));

    let result = runtime.apply(&PipelineCommand::InjectFault {
        command_id: pipeline_id(13),
        fault: FaultPoint::IntegrityHalt,
        recorded_at_ns: 12,
    });
    assert!(matches!(result, Err(Error::InjectedIntegrity)));
    assert!(runtime.is_halted());
}

#[test]
fn command_codec_is_exact_bounded_and_round_trips() {
    let command = PipelineCommand::InjectFault {
        command_id: pipeline_id(1),
        fault: FaultPoint::BeforeHandoff,
        recorded_at_ns: 9,
    };
    let encoded = encode_command(&command).expect("encode");
    assert_eq!(decode_command(&encoded).expect("decode"), command);
    let mut trailing = encoded;
    trailing.push(b'x');
    assert!(matches!(decode_command(&trailing), Err(Error::Json(_))));
}

fn rejected_hour(runtime: &mut PaperTradingRuntime, hour: u16, base: i64) {
    let wrapper = 100 + hour * 20;
    let value = 100 + hour;
    let approval = approve(runtime, value, wrapper, base);
    reserve(runtime, value, wrapper + 1, base + 1);
    let (request, permit) = authorize(runtime, value, approval, wrapper + 2, base + 2);
    submit(runtime, value, request, permit, wrapper + 4, base + 4);
    if hour % 2 == 0 {
        observe(
            runtime,
            value,
            wrapper + 5,
            0,
            base + 5,
            ExchangeEvent::Unknown {
                reason: UnknownReason::SubmitTimeout,
            },
        );
        observe(
            runtime,
            value,
            wrapper + 6,
            1,
            base + 6,
            ExchangeEvent::Rejected {
                class: RetryClass::Permanent,
                code: "paper_reject".to_owned(),
            },
        );
    } else {
        observe(
            runtime,
            value,
            wrapper + 5,
            0,
            base + 5,
            ExchangeEvent::Rejected {
                class: RetryClass::Permanent,
                code: "paper_reject".to_owned(),
            },
        );
    }
    runtime
        .apply(&wrap_accounting(
            wrapper + 7,
            base + 7,
            LedgerCommand::ReleaseReservation {
                command_id: LedgerCommandId(bytes(wrapper + 1_007)),
                reservation_id: ReservationId(bytes(value)),
                recorded_at_ns: base + 7,
            },
        ))
        .expect("release");
    let command = reconcile(wrapper + 8, runtime, u64::from(hour) + 2, base + 8);
    runtime.apply(&command).expect("hour reconcile");
}

#[test]
fn deterministic_multi_hour_soak_recovers_unknowns_without_leaking_capital() {
    let mut first = funded_ready();
    let mut second = funded_ready();
    for hour in 0..8 {
        rejected_hour(&mut first, hour, 100 + i64::from(hour) * 100);
        rejected_hour(&mut second, hour, 100 + i64::from(hour) * 100);
    }
    assert_eq!(first.snapshot().digest, second.snapshot().digest);
    assert_eq!(first.snapshot().reserved_order_count, 0);
    assert_eq!(first.ledger().snapshot().cash_available_micros, 1_000_000);
    assert!(first.reconciler().risk_gate().ready);
    assert!(!first.is_halted());
}

#[test]
fn partial_then_full_fill_rolls_into_the_next_hour_without_handoff_or_reservation_leak() {
    let mut runtime = funded_ready();
    let approval = approve(&mut runtime, 10, 10, 10);
    reserve(&mut runtime, 10, 11, 11);
    let (request, permit) = authorize(&mut runtime, 10, approval, 12, 12);
    submit(&mut runtime, 10, request, permit, 14, 14);
    observe(&mut runtime, 10, 15, 0, 15, ExchangeEvent::Acknowledged);

    for (index, wrapper, at, fill_id, ledger_id, cumulative, fully) in [
        (
            0_usize,
            16_u16,
            16_i64,
            "partial-a",
            780_u16,
            50_000_i128,
            false,
        ),
        (
            1_usize,
            19_u16,
            19_i64,
            "partial-b",
            781_u16,
            100_000_i128,
            true,
        ),
    ] {
        observe(
            &mut runtime,
            10,
            wrapper,
            u64::try_from(index + 1).expect("sequence"),
            at,
            ExchangeEvent::Match {
                fill: MatchFill {
                    fill_id: fill_id.to_owned(),
                    quantity_micros: 50_000,
                    consideration_micros: 20_000,
                    fee_micros: 500,
                    cumulative_quantity_micros: cumulative,
                    cumulative_consideration_micros: cumulative * 4 / 10,
                    cumulative_fee_micros: i128::try_from(index + 1).expect("fee") * 500,
                    ledger_command_id: LedgerCommandId(bytes(ledger_id)),
                },
                fully_matched: fully,
            },
        );
        runtime
            .apply(&PipelineCommand::RegisterHandoff {
                command_id: pipeline_id(wrapper + 1),
                order_id: order_id(10),
                handoff_index: index,
                recorded_at_ns: at + 1,
            })
            .expect("unique handoff");
        runtime
            .apply(&wrap_accounting(
                wrapper + 2,
                at + 2,
                LedgerCommand::ConfirmBuy {
                    command_id: LedgerCommandId(bytes(ledger_id)),
                    reservation_id: ReservationId(bytes(10)),
                    token: token("up"),
                    quantity_micros: 50_000,
                    consideration_micros: 20_000,
                    fee_micros: 500,
                    confirmation: format!("paper-{fill_id}"),
                    recorded_at_ns: at + 2,
                },
            ))
            .expect("partial ledger posting");
    }

    let (wrapper, at) = settle_all_handoffs(&mut runtime, 22, 22);
    let command = reconcile(wrapper, &runtime, 2, at);
    runtime.apply(&command).expect("filled hour reconcile");
    rejected_hour(&mut runtime, 1, 100);

    assert_eq!(runtime.snapshot().registered_handoff_count, 2);
    assert_eq!(runtime.snapshot().reserved_order_count, 0);
    assert_eq!(runtime.ledger().snapshot().cash_available_micros, 959_000);
    assert!(runtime.reconciler().risk_gate().ready);
    assert!(!runtime.is_halted());
}

#[test]
fn durable_journal_checkpoint_and_restart_replay_exact_state() {
    let directory = tempdir().expect("directory");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_records: 2,
            max_segment_bytes: 64 * 1024,
        },
    )
    .expect("writer");
    let recovery = RuntimeRecovery {
        runtime: PaperTradingRuntime::new(config()).expect("runtime"),
        last_sequence: None,
    };
    let mut durable = DurablePaperRuntime::new(writer, recovery).expect("durable");
    durable
        .apply(&wrap_accounting(
            1,
            1,
            LedgerCommand::FundCollateral {
                command_id: LedgerCommandId(bytes(1_001)),
                amount_micros: 1_000_000,
                recorded_at_ns: 1,
            },
        ))
        .expect("fund");
    let command = reconcile(2, durable.runtime(), 1, 2);
    durable.apply(&command).expect("reconcile");
    let expected = durable.runtime().snapshot().digest;
    let checkpoint = RuntimeCheckpoint {
        sequence: 1,
        runtime_digest: expected,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("write checkpoint");
    assert_eq!(read_checkpoint(&path).expect("read checkpoint"), checkpoint);
    drop(durable);

    let recovered = recover_segmented(&segments, config(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.runtime.snapshot().digest, expected);
    assert_eq!(recovered.last_sequence, Some(1));
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
            market_recorder::JournalError::Io(std::io::Error::other("injected sync failure")),
        ))
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}

#[test]
fn durable_sync_failure_never_installs_financial_state_and_poisons_owner() {
    let recovery = RuntimeRecovery {
        runtime: PaperTradingRuntime::new(config()).expect("runtime"),
        last_sequence: None,
    };
    let mut durable = DurablePaperRuntime::new(FailingJournal::default(), recovery).expect("owner");
    let command = wrap_accounting(
        1,
        1,
        LedgerCommand::FundCollateral {
            command_id: LedgerCommandId(bytes(1_001)),
            amount_micros: 1_000_000,
            recorded_at_ns: 1,
        },
    );
    assert!(matches!(
        durable.apply(&command),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(durable.runtime().snapshot().accepted_commands, 0);
    assert!(matches!(
        durable.apply(&command),
        Err(StorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn repeated_hour_profiles_are_digest_deterministic(hours in 1_u16..5) {
        let mut left = funded_ready();
        let mut right = funded_ready();
        for hour in 0..hours {
            let at = 100 + i64::from(hour) * 100;
            rejected_hour(&mut left, hour, at);
            rejected_hour(&mut right, hour, at);
        }
        prop_assert_eq!(left.snapshot().digest, right.snapshot().digest);
        prop_assert!(!left.is_halted());
    }
}
