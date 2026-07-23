use super::*;
use accounting_ledger::TokenKey;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use order_intent_policy::SignerPolicyFrame;
use paired_capital_staging::PairStageId;
use proptest::prelude::*;
use settlement_reconciliation::{ChainTokenBalance, FinalizedChainSnapshot, ReconcilerConfig};
use shadow_adapter_certification::{
    AdapterContract, CertificationCommand, CertificationCommandId, CertificationDetail, DryRunId,
    DryRunIntent, EligibilityAttestation, FailureId, FailureKind, FixtureId,
    OperationalObservation, ShadowAdapterCertification,
};
use tempfile::tempdir;

const BASE: i64 = 1_000_000;

const fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn token(value: &str) -> TokenKey {
    TokenKey::new("condition", value).expect("token")
}

fn adapter_contract() -> AdapterContract {
    AdapterContract {
        contract_id: bytes(1),
        venue: "polymarket-shadow".into(),
        rest_host: "https://clob.example.invalid".into(),
        websocket_host: "wss://ws.example.invalid".into(),
        chain_id: 137,
        exchange_contract: "exchange-v1".into(),
        settlement_contract: "ctf-v1".into(),
        schema_version: 1,
        required_regions: vec!["primary-us".into()],
        max_evidence_age_ns: 1_000,
        minimum_collateral_micros: 1_000_000,
        required_allowance_micros: 2_000_000,
        minimum_gas_micros: 100_000,
        max_relayer_queue_depth: 8,
        rules_digest: bytes(2),
        contract_digest: [0; 32],
    }
    .sealed()
}

fn signer_policy(valid_from_ns: i64, valid_until_ns: i64) -> SignerPolicyFrame {
    SignerPolicyFrame {
        policy_id: bytes(3),
        venue: "polymarket-shadow".into(),
        exchange_contract: "exchange-v1".into(),
        allowed_tokens: vec![token("up"), token("down")],
        max_quantity_micros: 1_000_000,
        max_price_micros: 900_000,
        max_notional_micros: 900_000,
        allow_maker: true,
        allow_taker: false,
        valid_from_ns,
        valid_until_ns,
    }
}

fn dry_intent(at: i64) -> DryRunIntent {
    DryRunIntent {
        venue: "polymarket-shadow".into(),
        exchange_contract: "exchange-v1".into(),
        token: token("up"),
        quantity_micros: 500_000,
        price_micros: 400_000,
        maker: true,
        evaluated_at_ns: at,
    }
}

#[allow(clippy::too_many_lines)]
fn certified_report() -> (AdapterContract, CertificationReport) {
    let contract = adapter_contract();
    let mut authority = ShadowAdapterCertification::default();
    let mut sequence = 1_u64;
    authority
        .apply(&CertificationCommand::RegisterContract {
            command_id: CertificationCommandId(bytes(1)),
            contract: contract.clone(),
            recorded_at_ns: BASE,
        })
        .expect("contract");
    let fixture_kinds = [
        FixtureKind::Restart425,
        FixtureKind::PostOnlyWindow,
        FixtureKind::CancelOnlyMode,
        FixtureKind::TakerDelay,
        FixtureKind::TickSizeChange,
        FixtureKind::RateLimit429,
        FixtureKind::UnknownOrder,
        FixtureKind::SettlementRetrying,
        FixtureKind::HeartbeatLost,
    ];
    for (offset, kind) in fixture_kinds.into_iter().enumerate() {
        let number = u8::try_from(offset + 2).expect("id");
        authority
            .apply(&CertificationCommand::RecordFixture {
                command_id: CertificationCommandId(bytes(number)),
                fixture: RecordedFixture {
                    fixture_id: FixtureId(bytes(number)),
                    contract_digest: contract.contract_digest,
                    sequence,
                    kind,
                    captured_at_ns: BASE + i64::from(number),
                    received_at_ns: BASE + i64::from(number),
                    payload_digest: bytes(number.saturating_add(40)),
                },
                recorded_at_ns: BASE + i64::from(number),
            })
            .expect("fixture");
        sequence += 1;
    }
    authority
        .apply(&CertificationCommand::ObserveEligibility {
            command_id: CertificationCommandId(bytes(11)),
            attestation: EligibilityAttestation {
                sequence: 1,
                region: "primary-us".into(),
                egress_fingerprint: bytes(11),
                eligible: true,
                checked_at_ns: BASE + 20,
                valid_until_ns: BASE + 200,
                source_digest: bytes(61),
            },
            recorded_at_ns: BASE + 20,
        })
        .expect("eligibility");
    authority
        .apply(&CertificationCommand::ObserveOperational {
            command_id: CertificationCommandId(bytes(12)),
            observation: OperationalObservation {
                sequence: 1,
                wallet_alias: "paper-safe".into(),
                chain_id: 137,
                collateral_micros: 3_000_000,
                allowance_micros: 2_000_000,
                gas_micros: 100_000,
                relayer_available: true,
                relayer_queue_depth: 0,
                observed_at_ns: BASE + 21,
                valid_until_ns: BASE + 200,
                observation_digest: [0; 32],
            }
            .sealed(),
            recorded_at_ns: BASE + 21,
        })
        .expect("operational");
    let baseline_policy = signer_policy(BASE, BASE + 200);
    let baseline = dry_intent(BASE + 22);
    let mut wrong_contract = baseline.clone();
    wrong_contract.exchange_contract = "wrong".into();
    let mut wrong_token = baseline.clone();
    wrong_token.token = token("other");
    let mut excessive = baseline.clone();
    excessive.quantity_micros = 1_000_001;
    for (offset, (policy, intent)) in [
        (baseline_policy.clone(), baseline),
        (baseline_policy.clone(), wrong_contract),
        (baseline_policy.clone(), wrong_token),
        (baseline_policy, excessive),
        (signer_policy(BASE + 100, BASE + 300), dry_intent(BASE + 26)),
    ]
    .into_iter()
    .enumerate()
    {
        let number = u8::try_from(13 + offset).expect("id");
        authority
            .apply(&CertificationCommand::DryRunSigner {
                command_id: CertificationCommandId(bytes(number)),
                dry_run_id: DryRunId(bytes(number)),
                policy,
                intent,
                recorded_at_ns: BASE + 22 + i64::try_from(offset).expect("time"),
            })
            .expect("dry run");
    }
    let failures = [
        FailureKind::AllowanceInsufficient,
        FailureKind::GasInsufficient,
        FailureKind::RelayerUnavailable,
        FailureKind::EligibilityBlocked,
        FailureKind::UnknownSubmission,
        FailureKind::EngineRestarting,
        FailureKind::RateLimited,
        FailureKind::SettlementRetrying,
    ];
    for (offset, kind) in failures.into_iter().enumerate() {
        let number = u8::try_from(18 + offset).expect("id");
        authority
            .apply(&CertificationCommand::SimulateFailure {
                command_id: CertificationCommandId(bytes(number)),
                failure_id: FailureId(bytes(number)),
                kind,
                recorded_at_ns: BASE + 30 + i64::try_from(offset).expect("time"),
            })
            .expect("failure");
    }
    let outcome = authority
        .apply(&CertificationCommand::Evaluate {
            command_id: CertificationCommandId(bytes(27)),
            profile_id: bytes(90),
            evaluated_at_ns: BASE + 50,
            recorded_at_ns: BASE + 50,
        })
        .expect("evaluate");
    let CertificationDetail::Evaluated(report) = outcome.detail else {
        panic!("report")
    };
    assert_eq!(report.status, CertificationStatus::Certified);
    (contract, report)
}

fn config(contract: &AdapterContract) -> GatewayConfig {
    GatewayConfig {
        expected_contract_digest: contract.contract_digest,
        certification_max_age_ns: 100,
        heartbeat_max_age_ns: 20,
        mode_validity_ns: 50,
    }
}

fn reconciliation() -> ReconcilerConfig {
    ReconcilerConfig {
        chain_id: 137,
        wallet: "paper-wallet".into(),
        confirmation_grace_ns: 1_000,
        max_intents: 64,
        max_tokens: 16,
    }
}

fn heartbeat(sequence: u64, at: i64, healthy: bool) -> StackHeartbeat {
    StackHeartbeat {
        sequence,
        strategy_healthy: healthy,
        risk_healthy: healthy,
        market_feed_healthy: healthy,
        user_feed_healthy: healthy,
        ledger_reconciled: healthy,
        observed_at_ns: at,
        valid_until_ns: at + 20,
        observation_digest: [0; 32],
    }
    .sealed()
}

#[allow(clippy::needless_pass_by_value)]
fn apply(harness: &mut ShadowGatewayHarness, command: GatewayCommand) -> GatewayOutcome {
    harness.apply(&command).expect("gateway command")
}

fn install_ready(harness: &mut ShadowGatewayHarness, report: &CertificationReport, at: i64) {
    apply(
        harness,
        GatewayCommand::InstallCertification {
            command_id: GatewayCommandId(bytes(100)),
            report: report.clone(),
            recorded_at_ns: at,
        },
    );
    apply(
        harness,
        GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(101)),
            heartbeat: heartbeat(1, at + 1, true),
            recorded_at_ns: at + 1,
        },
    );
    assert!(harness.snapshot().new_shadow_exposure_allowed);
}

fn exposure_command(id: u8, at: i64) -> UnifiedCommand {
    UnifiedCommand::AuthorizeAndSubmitFirst {
        command_id: UnifiedCommandId(bytes(id)),
        stage_id: PairStageId(bytes(77)),
        leg_index: 0,
        max_mode_age_ns: 20,
        valid_until_ns: at + 10,
        local_submission_id: "paper-only".into(),
        recorded_at_ns: at,
    }
}

fn exact_chain(harness: &ShadowGatewayHarness, block: u64, at: i64) -> FinalizedChainSnapshot {
    let view = harness
        .runtime()
        .ctf()
        .parent()
        .ledger_reconciliation_view();
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

fn fixture(
    contract: &AdapterContract,
    id: u8,
    sequence: u64,
    kind: FixtureKind,
    at: i64,
) -> RecordedFixture {
    RecordedFixture {
        fixture_id: FixtureId(bytes(id)),
        contract_digest: contract.contract_digest,
        sequence,
        kind,
        captured_at_ns: at,
        received_at_ns: at,
        payload_digest: bytes(id.saturating_add(100)),
    }
}

#[test]
fn fresh_certification_and_complete_heartbeat_are_both_required() {
    let (contract, report) = certified_report();
    let mut harness = ShadowGatewayHarness::new(config(&contract), reconciliation()).expect("new");
    apply(
        &mut harness,
        GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(40)),
            heartbeat: heartbeat(1, BASE + 50, true),
            recorded_at_ns: BASE + 50,
        },
    );
    assert!(!harness.snapshot().new_shadow_exposure_allowed);
    let outcome = apply(
        &mut harness,
        GatewayCommand::ApplyRuntime {
            command_id: GatewayCommandId(bytes(41)),
            command: Box::new(exposure_command(41, BASE + 51)),
            recorded_at_ns: BASE + 51,
        },
    );
    assert_eq!(
        outcome.detail,
        GatewayDetail::RuntimeDenied {
            reason: GatewayDenial::CertificationMissing,
            derived: Vec::new(),
        }
    );
    apply(
        &mut harness,
        GatewayCommand::InstallCertification {
            command_id: GatewayCommandId(bytes(42)),
            report,
            recorded_at_ns: BASE + 52,
        },
    );
    assert!(harness.snapshot().new_shadow_exposure_allowed);
    assert!(!harness.snapshot().authority_granted);
}

#[test]
fn expiry_boundary_immediately_disables_new_shadow_exposure() {
    let (contract, report) = certified_report();
    let mut harness = ShadowGatewayHarness::new(config(&contract), reconciliation()).expect("new");
    install_ready(&mut harness, &report, BASE + 50);
    apply(
        &mut harness,
        GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(102)),
            heartbeat: heartbeat(2, BASE + 150, true),
            recorded_at_ns: BASE + 150,
        },
    );
    assert!(harness.snapshot().new_shadow_exposure_allowed);
    let tick = apply(
        &mut harness,
        GatewayCommand::Tick {
            command_id: GatewayCommandId(bytes(103)),
            now_ns: BASE + 151,
            recorded_at_ns: BASE + 151,
        },
    );
    assert!(matches!(
        tick.detail,
        GatewayDetail::TickApplied {
            certification_expired: true,
            ..
        }
    ));
    assert_eq!(harness.snapshot().mode, GatewayMode::CertificationExpired);
    assert!(!harness.snapshot().new_shadow_exposure_allowed);
}

#[test]
fn unhealthy_heartbeat_triggers_dead_man_without_releasing_backing() {
    let (contract, report) = certified_report();
    let mut harness = ShadowGatewayHarness::new(config(&contract), reconciliation()).expect("new");
    install_ready(&mut harness, &report, BASE + 50);
    let before = harness.snapshot().nested_cash_reserved_micros;
    let outcome = apply(
        &mut harness,
        GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(104)),
            heartbeat: heartbeat(2, BASE + 52, false),
            recorded_at_ns: BASE + 52,
        },
    );
    assert!(matches!(
        outcome.detail,
        GatewayDetail::HeartbeatObserved {
            dead_man_triggered: true,
            ..
        }
    ));
    assert_eq!(harness.snapshot().mode, GatewayMode::DeadManTriggered);
    assert_eq!(harness.snapshot().nested_cash_reserved_micros, before);
    assert!(!harness.snapshot().new_shadow_exposure_allowed);
}

#[test]
fn every_fixture_translates_without_retry_or_backing_release() {
    let (contract, report) = certified_report();
    let mut harness = ShadowGatewayHarness::new(config(&contract), reconciliation()).expect("new");
    install_ready(&mut harness, &report, BASE + 50);
    let kinds = [
        FixtureKind::Restart425,
        FixtureKind::PostOnlyWindow,
        FixtureKind::CancelOnlyMode,
        FixtureKind::TakerDelay,
        FixtureKind::TickSizeChange,
        FixtureKind::RateLimit429,
        FixtureKind::UnknownOrder,
        FixtureKind::SettlementRetrying,
        FixtureKind::HeartbeatLost,
    ];
    for (offset, kind) in kinds.into_iter().enumerate() {
        let number = u8::try_from(110 + offset).expect("id");
        let at = BASE + 53 + i64::try_from(offset).expect("time");
        let outcome = apply(
            &mut harness,
            GatewayCommand::ApplyFixture {
                command_id: GatewayCommandId(bytes(number)),
                fixture: fixture(
                    &contract,
                    number,
                    u64::try_from(offset + 1).expect("sequence"),
                    kind,
                    at,
                ),
                recorded_at_ns: at,
            },
        );
        assert!(matches!(
            outcome.detail,
            GatewayDetail::FixtureApplied {
                automatic_retry: false,
                backing_released: false,
                ..
            }
        ));
    }
    assert_eq!(harness.snapshot().fixture_count, 9);
    assert_eq!(harness.snapshot().mode, GatewayMode::DeadManTriggered);
}

#[test]
fn restart_recovery_requires_all_evidence_before_normal_mode() {
    let (contract, report) = certified_report();
    let mut harness = ShadowGatewayHarness::new(config(&contract), reconciliation()).expect("new");
    install_ready(&mut harness, &report, BASE + 50);
    apply(
        &mut harness,
        GatewayCommand::ApplyRuntime {
            command_id: GatewayCommandId(bytes(118)),
            command: Box::new(UnifiedCommand::Fund {
                command_id: UnifiedCommandId(bytes(118)),
                amount_micros: 1_000_000,
                recorded_at_ns: BASE + 52,
            }),
            recorded_at_ns: BASE + 52,
        },
    );
    let chain = exact_chain(&harness, 1, BASE + 53);
    apply(
        &mut harness,
        GatewayCommand::ApplyRuntime {
            command_id: GatewayCommandId(bytes(119)),
            command: Box::new(UnifiedCommand::Reconcile {
                command_id: UnifiedCommandId(bytes(119)),
                chain,
                evaluated_at_ns: BASE + 53,
                recorded_at_ns: BASE + 53,
            }),
            recorded_at_ns: BASE + 53,
        },
    );
    apply(
        &mut harness,
        GatewayCommand::ApplyFixture {
            command_id: GatewayCommandId(bytes(120)),
            fixture: fixture(&contract, 120, 1, FixtureKind::Restart425, BASE + 54),
            recorded_at_ns: BASE + 54,
        },
    );
    let denied = apply(
        &mut harness,
        GatewayCommand::Recover {
            command_id: GatewayCommandId(bytes(121)),
            recovery_epoch: 1,
            reconciliation_current: true,
            unknown_orders_cleared: false,
            recovery_evidence_digest: bytes(121),
            recorded_at_ns: BASE + 55,
        },
    );
    assert_eq!(
        denied.detail,
        GatewayDetail::RecoveryDenied(GatewayDenial::RecoveryEvidenceMissing)
    );
    let recovered = apply(
        &mut harness,
        GatewayCommand::Recover {
            command_id: GatewayCommandId(bytes(122)),
            recovery_epoch: 1,
            reconciliation_current: true,
            unknown_orders_cleared: true,
            recovery_evidence_digest: bytes(122),
            recorded_at_ns: BASE + 56,
        },
    );
    assert!(matches!(recovered.detail, GatewayDetail::Recovered { .. }));
    assert_eq!(harness.snapshot().mode, GatewayMode::Ready);
}

#[test]
fn caller_cannot_inject_exchange_mode() {
    let (contract, report) = certified_report();
    let mut harness = ShadowGatewayHarness::new(config(&contract), reconciliation()).expect("new");
    install_ready(&mut harness, &report, BASE + 50);
    let outcome = apply(
        &mut harness,
        GatewayCommand::ApplyRuntime {
            command_id: GatewayCommandId(bytes(130)),
            command: Box::new(UnifiedCommand::ObserveMode {
                command_id: UnifiedCommandId(bytes(130)),
                observation: ExchangeModeObservation {
                    sequence: 99,
                    mode: ExchangeMode::Normal,
                    observed_at_ns: BASE + 52,
                    valid_until_ns: BASE + 100,
                },
                recorded_at_ns: BASE + 52,
            }),
            recorded_at_ns: BASE + 52,
        },
    );
    assert_eq!(
        outcome.detail,
        GatewayDetail::RuntimeDenied {
            reason: GatewayDenial::CallerModeObservationForbidden,
            derived: Vec::new(),
        }
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
            market_recorder::JournalError::Io(std::io::Error::other("sync failure")),
        ))
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}

#[test]
fn durable_replay_checkpoint_and_sync_failure_are_fail_closed() {
    let (contract, report) = certified_report();
    let gateway_config = config(&contract);
    let directory = tempdir().expect("dir");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 512 * 1024,
            max_segment_records: 2,
        },
    )
    .expect("writer");
    let recovery = GatewayRecovery {
        gateway: ShadowGatewayHarness::new(gateway_config.clone(), reconciliation()).expect("new"),
        last_sequence: None,
    };
    let mut durable = DurableShadowGateway::new(writer, recovery).expect("durable");
    durable
        .apply(&GatewayCommand::InstallCertification {
            command_id: GatewayCommandId(bytes(140)),
            report,
            recorded_at_ns: BASE + 50,
        })
        .expect("install");
    durable
        .apply(&GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(141)),
            heartbeat: heartbeat(1, BASE + 51, true),
            recorded_at_ns: BASE + 51,
        })
        .expect("heartbeat");
    let expected = durable.gateway().snapshot().digest;
    let checkpoint = GatewayCheckpoint {
        sequence: 1,
        gateway_digest: expected,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(
        &segments,
        gateway_config.clone(),
        reconciliation(),
        Some(checkpoint),
    )
    .expect("recover");
    assert_eq!(recovered.gateway.snapshot().digest, expected);

    let recovery = GatewayRecovery {
        gateway: ShadowGatewayHarness::new(gateway_config, reconciliation()).expect("new"),
        last_sequence: None,
    };
    let mut failing = DurableShadowGateway::new(FailingJournal::default(), recovery).expect("new");
    let command = GatewayCommand::ObserveHeartbeat {
        command_id: GatewayCommandId(bytes(142)),
        heartbeat: heartbeat(1, BASE + 50, true),
        recorded_at_ns: BASE + 50,
    };
    assert!(matches!(
        failing.apply(&command),
        Err(StorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&command),
        Err(StorageError::Halted(_))
    ));
    assert_eq!(failing.gateway().snapshot().accepted_commands, 0);
}

#[test]
fn twenty_four_heartbeat_cycles_have_no_capital_drift() {
    let (contract, report) = certified_report();
    let mut gateway_config = config(&contract);
    gateway_config.certification_max_age_ns = 10_000;
    gateway_config.heartbeat_max_age_ns = 100;
    let mut harness = ShadowGatewayHarness::new(gateway_config, reconciliation()).expect("new");
    install_ready(&mut harness, &report, BASE + 50);
    let initial_cash = harness.snapshot().nested_cash_reserved_micros;
    for cycle in 2_u8..=25 {
        let at = BASE + 50 + i64::from(cycle);
        apply(
            &mut harness,
            GatewayCommand::ObserveHeartbeat {
                command_id: GatewayCommandId(bytes(cycle)),
                heartbeat: StackHeartbeat {
                    sequence: u64::from(cycle),
                    strategy_healthy: true,
                    risk_healthy: true,
                    market_feed_healthy: true,
                    user_feed_healthy: true,
                    ledger_reconciled: true,
                    observed_at_ns: at,
                    valid_until_ns: at + 100,
                    observation_digest: [0; 32],
                }
                .sealed(),
                recorded_at_ns: at,
            },
        );
    }
    assert_eq!(harness.snapshot().nested_cash_reserved_micros, initial_cash);
    assert!(harness.snapshot().new_shadow_exposure_allowed);
    assert_eq!(harness.snapshot().mode_sequence, 26);
}

proptest! {
    #[test]
    fn certification_older_than_limit_never_allows_exposure(extra in 1_i64..1_000_000) {
        let (contract, report) = certified_report();
        let gateway_config = config(&contract);
        let at = report.evaluated_at_ns + gateway_config.certification_max_age_ns + extra;
        prop_assert_eq!(
            certification_denial(&gateway_config, Some(&report), at),
            Some(GatewayDenial::CertificationExpired)
        );
    }
}
