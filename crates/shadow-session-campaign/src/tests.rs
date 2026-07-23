use super::*;
use accounting_ledger::TokenKey;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use order_intent_policy::SignerPolicyFrame;
use proptest::prelude::*;
use settlement_reconciliation::{FinalizedChainSnapshot, ReconcilerConfig};
use shadow_adapter_certification::{
    AdapterContract, CertificationCommand, CertificationCommandId, CertificationDetail,
    CertificationReport, DryRunId, DryRunIntent, EligibilityAttestation, FailureId, FailureKind,
    FixtureId, FixtureKind, OperationalObservation, RecordedFixture, ShadowAdapterCertification,
};
use shadow_gateway_harness::{GatewayCommandId, StackHeartbeat};
use tempfile::tempdir;
use unified_paired_trading_runtime::{UnifiedCommand, UnifiedCommandId};

const DAY: i64 = 86_400_000_000_000;
const START: i64 = 1_000_000;

const fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn token(value: &str) -> TokenKey {
    TokenKey::new("condition", value).expect("token")
}

fn contract() -> AdapterContract {
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

fn signer_policy(from: i64, until: i64) -> SignerPolicyFrame {
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
        valid_from_ns: from,
        valid_until_ns: until,
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
fn certified_report(evaluated_at: i64, profile: u8) -> CertificationReport {
    let base = evaluated_at - 50;
    let contract = contract();
    let mut authority = ShadowAdapterCertification::default();
    authority
        .apply(&CertificationCommand::RegisterContract {
            command_id: CertificationCommandId(bytes(1)),
            contract: contract.clone(),
            recorded_at_ns: base,
        })
        .expect("contract");
    for (offset, kind) in [
        FixtureKind::Restart425,
        FixtureKind::PostOnlyWindow,
        FixtureKind::CancelOnlyMode,
        FixtureKind::TakerDelay,
        FixtureKind::TickSizeChange,
        FixtureKind::RateLimit429,
        FixtureKind::UnknownOrder,
        FixtureKind::SettlementRetrying,
        FixtureKind::HeartbeatLost,
    ]
    .into_iter()
    .enumerate()
    {
        let number = u8::try_from(offset + 2).expect("id");
        authority
            .apply(&CertificationCommand::RecordFixture {
                command_id: CertificationCommandId(bytes(number)),
                fixture: RecordedFixture {
                    fixture_id: FixtureId(bytes(number)),
                    contract_digest: contract.contract_digest,
                    sequence: u64::try_from(offset + 1).expect("sequence"),
                    kind,
                    captured_at_ns: base + i64::from(number),
                    received_at_ns: base + i64::from(number),
                    payload_digest: bytes(number.saturating_add(40)),
                },
                recorded_at_ns: base + i64::from(number),
            })
            .expect("fixture");
    }
    authority
        .apply(&CertificationCommand::ObserveEligibility {
            command_id: CertificationCommandId(bytes(11)),
            attestation: EligibilityAttestation {
                sequence: 1,
                region: "primary-us".into(),
                egress_fingerprint: bytes(11),
                eligible: true,
                checked_at_ns: base + 20,
                valid_until_ns: evaluated_at + 100,
                source_digest: bytes(61),
            },
            recorded_at_ns: base + 20,
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
                observed_at_ns: base + 21,
                valid_until_ns: evaluated_at + 100,
                observation_digest: [0; 32],
            }
            .sealed(),
            recorded_at_ns: base + 21,
        })
        .expect("operational");
    let policy = signer_policy(base, evaluated_at + 100);
    let baseline = dry_intent(base + 22);
    let mut wrong_contract = baseline.clone();
    wrong_contract.exchange_contract = "wrong".into();
    let mut wrong_token = baseline.clone();
    wrong_token.token = token("other");
    let mut excessive = baseline.clone();
    excessive.quantity_micros = 1_000_001;
    for (offset, (dry_policy, intent)) in [
        (policy.clone(), baseline),
        (policy.clone(), wrong_contract),
        (policy.clone(), wrong_token),
        (policy, excessive),
        (
            signer_policy(evaluated_at + 1, evaluated_at + 200),
            dry_intent(base + 26),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let number = u8::try_from(13 + offset).expect("id");
        authority
            .apply(&CertificationCommand::DryRunSigner {
                command_id: CertificationCommandId(bytes(number)),
                dry_run_id: DryRunId(bytes(number)),
                policy: dry_policy,
                intent,
                recorded_at_ns: base + 22 + i64::try_from(offset).expect("time"),
            })
            .expect("dry run");
    }
    for (offset, kind) in [
        FailureKind::AllowanceInsufficient,
        FailureKind::GasInsufficient,
        FailureKind::RelayerUnavailable,
        FailureKind::EligibilityBlocked,
        FailureKind::UnknownSubmission,
        FailureKind::EngineRestarting,
        FailureKind::RateLimited,
        FailureKind::SettlementRetrying,
    ]
    .into_iter()
    .enumerate()
    {
        let number = u8::try_from(18 + offset).expect("id");
        authority
            .apply(&CertificationCommand::SimulateFailure {
                command_id: CertificationCommandId(bytes(number)),
                failure_id: FailureId(bytes(number)),
                kind,
                recorded_at_ns: base + 30 + i64::try_from(offset).expect("time"),
            })
            .expect("failure");
    }
    let outcome = authority
        .apply(&CertificationCommand::Evaluate {
            command_id: CertificationCommandId(bytes(27)),
            profile_id: bytes(profile),
            evaluated_at_ns: evaluated_at,
            recorded_at_ns: evaluated_at,
        })
        .expect("evaluate");
    let CertificationDetail::Evaluated(report) = outcome.detail else {
        panic!("report")
    };
    report
}

fn campaign_policy() -> CampaignPolicy {
    CampaignPolicy {
        max_sessions: 8,
        max_steps: 128,
        minimum_duration_ns: 2 * DAY,
        minimum_sessions: 2,
        maximum_step_gap_ns: DAY,
    }
}

fn gateway_config() -> GatewayConfig {
    GatewayConfig {
        expected_contract_digest: contract().contract_digest,
        certification_max_age_ns: 10,
        heartbeat_max_age_ns: 3 * DAY,
        mode_validity_ns: 3 * DAY,
    }
}

fn reconciliation() -> ReconcilerConfig {
    ReconcilerConfig {
        chain_id: 137,
        wallet: "paper-wallet".into(),
        confirmation_grace_ns: DAY,
        max_intents: 64,
        max_tokens: 16,
    }
}

fn heartbeat(sequence: u64, at: i64, market: bool, user: bool) -> StackHeartbeat {
    StackHeartbeat {
        sequence,
        strategy_healthy: true,
        risk_healthy: true,
        market_feed_healthy: market,
        user_feed_healthy: user,
        ledger_reconciled: true,
        observed_at_ns: at,
        valid_until_ns: at + 3 * DAY,
        observation_digest: [0; 32],
    }
    .sealed()
}

fn fixture(id: u8, sequence: u64, kind: FixtureKind, at: i64) -> RecordedFixture {
    RecordedFixture {
        fixture_id: FixtureId(bytes(id)),
        contract_digest: contract().contract_digest,
        sequence,
        kind,
        captured_at_ns: at,
        received_at_ns: at,
        payload_digest: bytes(id.saturating_add(100)),
    }
}

fn push_step(steps: &mut Vec<CampaignStep>, at: i64, action: CampaignAction) {
    let previous = steps.last().map_or([0; 32], |step| step.step_digest);
    let sequence = u64::try_from(steps.len() + 1).expect("sequence");
    steps.push(
        CampaignStep {
            sequence,
            scheduled_at_ns: at,
            previous_step_digest: previous,
            action,
            step_digest: [0; 32],
        }
        .sealed(),
    );
}

fn gateway_action(command: GatewayCommand) -> CampaignAction {
    CampaignAction::Gateway {
        session_id: None,
        command: Box::new(command),
    }
}

#[allow(clippy::too_many_lines)]
fn complete_campaign_commands() -> Vec<CampaignCommand> {
    let end = START + 2 * DAY;
    let first = bytes(70);
    let second = bytes(71);
    let sessions = vec![
        RecordedSession {
            session_id: first,
            start_ns: START,
            end_ns: START + DAY,
            recording_digest: bytes(80),
        },
        RecordedSession {
            session_id: second,
            start_ns: START + DAY,
            end_ns: end,
            recording_digest: bytes(81),
        },
    ];
    let mut steps = Vec::new();
    push_step(
        &mut steps,
        START,
        gateway_action(GatewayCommand::InstallCertification {
            command_id: GatewayCommandId(bytes(1)),
            report: certified_report(START, 90),
            recorded_at_ns: START,
        }),
    );
    push_step(
        &mut steps,
        START,
        gateway_action(GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(2)),
            heartbeat: heartbeat(1, START, true, true),
            recorded_at_ns: START,
        }),
    );
    push_step(
        &mut steps,
        START,
        CampaignAction::OpenSession { session_id: first },
    );
    push_step(
        &mut steps,
        START + 1,
        CampaignAction::Gateway {
            session_id: Some(first),
            command: Box::new(GatewayCommand::ApplyRuntime {
                command_id: GatewayCommandId(bytes(3)),
                command: Box::new(UnifiedCommand::Fund {
                    command_id: UnifiedCommandId(bytes(3)),
                    amount_micros: 1_000_000,
                    recorded_at_ns: START + 1,
                }),
                recorded_at_ns: START + 1,
            }),
        },
    );
    push_step(
        &mut steps,
        START + 2,
        CampaignAction::Gateway {
            session_id: Some(first),
            command: Box::new(GatewayCommand::ApplyRuntime {
                command_id: GatewayCommandId(bytes(4)),
                command: Box::new(UnifiedCommand::Reconcile {
                    command_id: UnifiedCommandId(bytes(4)),
                    chain: FinalizedChainSnapshot {
                        chain_id: 137,
                        wallet: "paper-wallet".into(),
                        block_number: 1,
                        block_hash: "block-1".into(),
                        finalized_at_ns: START + 2,
                        observed_at_ns: START + 2,
                        collateral_micros: 1_000_000,
                        token_balances: Vec::new(),
                    },
                    evaluated_at_ns: START + 2,
                    recorded_at_ns: START + 2,
                }),
                recorded_at_ns: START + 2,
            }),
        },
    );
    push_step(
        &mut steps,
        START + 3,
        gateway_action(GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(5)),
            heartbeat: heartbeat(2, START + 3, false, false),
            recorded_at_ns: START + 3,
        }),
    );
    push_step(
        &mut steps,
        START + 4,
        gateway_action(GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(6)),
            heartbeat: heartbeat(3, START + 4, true, true),
            recorded_at_ns: START + 4,
        }),
    );
    push_step(
        &mut steps,
        START + 11,
        gateway_action(GatewayCommand::Tick {
            command_id: GatewayCommandId(bytes(7)),
            now_ns: START + 11,
            recorded_at_ns: START + 11,
        }),
    );
    push_step(
        &mut steps,
        START + 11,
        gateway_action(GatewayCommand::InstallCertification {
            command_id: GatewayCommandId(bytes(8)),
            report: certified_report(START + 11, 91),
            recorded_at_ns: START + 11,
        }),
    );
    push_step(
        &mut steps,
        START + 11,
        gateway_action(GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(9)),
            heartbeat: heartbeat(4, START + 11, true, true),
            recorded_at_ns: START + 11,
        }),
    );
    push_step(
        &mut steps,
        START + 12,
        gateway_action(GatewayCommand::ApplyFixture {
            command_id: GatewayCommandId(bytes(10)),
            fixture: fixture(10, 1, FixtureKind::Restart425, START + 12),
            recorded_at_ns: START + 12,
        }),
    );
    push_step(
        &mut steps,
        START + 13,
        gateway_action(GatewayCommand::ApplyFixture {
            command_id: GatewayCommandId(bytes(11)),
            fixture: fixture(11, 2, FixtureKind::UnknownOrder, START + 13),
            recorded_at_ns: START + 13,
        }),
    );
    push_step(
        &mut steps,
        START + 14,
        gateway_action(GatewayCommand::Recover {
            command_id: GatewayCommandId(bytes(12)),
            recovery_epoch: 1,
            reconciliation_current: true,
            unknown_orders_cleared: true,
            recovery_evidence_digest: bytes(12),
            recorded_at_ns: START + 14,
        }),
    );
    push_step(
        &mut steps,
        START + 15,
        gateway_action(GatewayCommand::ApplyFixture {
            command_id: GatewayCommandId(bytes(13)),
            fixture: fixture(13, 3, FixtureKind::HeartbeatLost, START + 15),
            recorded_at_ns: START + 15,
        }),
    );
    push_step(
        &mut steps,
        START + 16,
        gateway_action(GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(14)),
            heartbeat: heartbeat(5, START + 16, true, true),
            recorded_at_ns: START + 16,
        }),
    );
    push_step(
        &mut steps,
        START + DAY,
        CampaignAction::CloseSession {
            session_id: first,
            replay_digest: bytes(80),
        },
    );
    push_step(
        &mut steps,
        START + DAY,
        CampaignAction::OpenSession { session_id: second },
    );
    push_step(
        &mut steps,
        end,
        CampaignAction::CloseSession {
            session_id: second,
            replay_digest: bytes(81),
        },
    );
    push_step(
        &mut steps,
        end,
        gateway_action(GatewayCommand::InstallCertification {
            command_id: GatewayCommandId(bytes(15)),
            report: certified_report(end, 92),
            recorded_at_ns: end,
        }),
    );
    push_step(
        &mut steps,
        end,
        gateway_action(GatewayCommand::ObserveHeartbeat {
            command_id: GatewayCommandId(bytes(16)),
            heartbeat: heartbeat(6, end, true, true),
            recorded_at_ns: end,
        }),
    );
    let manifest = CampaignManifest {
        campaign_id: bytes(60),
        start_ns: START,
        end_ns: end,
        sessions,
        required_scenarios: vec![
            RequiredScenario::CertificationRenewal,
            RequiredScenario::CertificationExpiry,
            RequiredScenario::MarketPartition,
            RequiredScenario::UserPartition,
            RequiredScenario::DeadMan,
            RequiredScenario::HeartbeatLoss,
            RequiredScenario::Restart,
            RequiredScenario::UnknownStateRecovery,
        ],
        expected_step_count: u64::try_from(steps.len()).expect("count"),
        expected_schedule_digest: steps.last().expect("step").step_digest,
        manifest_digest: [0; 32],
    }
    .sealed();
    let mut commands = vec![CampaignCommand::Register {
        command_id: CampaignCommandId(bytes(1)),
        manifest,
        recorded_at_ns: START - 1,
    }];
    commands.extend(steps.into_iter().enumerate().map(|(index, step)| {
        CampaignCommand::ApplyStep {
            command_id: CampaignCommandId(bytes(u8::try_from(index + 2).expect("id"))),
            campaign_id: bytes(60),
            recorded_at_ns: step.scheduled_at_ns,
            step: Box::new(step),
        }
    }));
    commands.push(CampaignCommand::Finalize {
        command_id: CampaignCommandId(bytes(250)),
        campaign_id: bytes(60),
        bundle_id: bytes(61),
        evaluated_at_ns: end,
        recorded_at_ns: end,
    });
    commands
}

fn new_campaign() -> ShadowSessionCampaign {
    ShadowSessionCampaign::new(campaign_policy(), gateway_config(), reconciliation()).expect("new")
}

fn run_all(campaign: &mut ShadowSessionCampaign, commands: &[CampaignCommand]) -> CampaignOutcome {
    let mut last = None;
    for command in commands {
        last = Some(campaign.apply(command).expect("command"));
    }
    last.expect("outcome")
}

#[test]
fn complete_two_day_campaign_produces_non_authorizing_eligible_evidence() {
    let commands = complete_campaign_commands();
    let mut campaign = new_campaign();
    let outcome = run_all(&mut campaign, &commands);
    let CampaignDetail::Finalized(bundle) = outcome.detail else {
        panic!("bundle")
    };
    assert_eq!(bundle.status, CampaignStatus::PromotionEligible);
    assert!(bundle.reasons.is_empty());
    assert_eq!(bundle.session_count, 2);
    assert_eq!(bundle.completed_session_count, 2);
    assert_eq!(bundle.covered_scenarios.len(), 8);
    assert!(bundle.operator_decision_required);
    assert!(!bundle.promotion_authority_granted);
    assert!(!bundle.deployment_authority_granted);
    assert!(bundle.verify_digest());
    assert_eq!(bundle.final_cash_reserved_micros, 0);
}

#[test]
fn missing_required_scenario_is_attributable_and_not_eligible() {
    let first = bytes(30);
    let second = bytes(31);
    let end = START + 2 * DAY;
    let sessions = vec![
        RecordedSession {
            session_id: first,
            start_ns: START,
            end_ns: START + DAY,
            recording_digest: bytes(32),
        },
        RecordedSession {
            session_id: second,
            start_ns: START + DAY,
            end_ns: end,
            recording_digest: bytes(33),
        },
    ];
    let mut steps = Vec::new();
    push_step(
        &mut steps,
        START,
        CampaignAction::OpenSession { session_id: first },
    );
    push_step(
        &mut steps,
        START + DAY,
        CampaignAction::CloseSession {
            session_id: first,
            replay_digest: bytes(32),
        },
    );
    push_step(
        &mut steps,
        START + DAY,
        CampaignAction::OpenSession { session_id: second },
    );
    push_step(
        &mut steps,
        end,
        CampaignAction::CloseSession {
            session_id: second,
            replay_digest: bytes(33),
        },
    );
    let manifest = CampaignManifest {
        campaign_id: bytes(34),
        start_ns: START,
        end_ns: end,
        sessions,
        required_scenarios: vec![RequiredScenario::DeadMan],
        expected_step_count: 4,
        expected_schedule_digest: steps.last().expect("step").step_digest,
        manifest_digest: [0; 32],
    }
    .sealed();
    let mut campaign = new_campaign();
    campaign
        .apply(&CampaignCommand::Register {
            command_id: CampaignCommandId(bytes(35)),
            manifest,
            recorded_at_ns: START - 1,
        })
        .expect("register");
    for (index, step) in steps.into_iter().enumerate() {
        let at = step.scheduled_at_ns;
        campaign
            .apply(&CampaignCommand::ApplyStep {
                command_id: CampaignCommandId(bytes(u8::try_from(36 + index).expect("command id"))),
                campaign_id: bytes(34),
                step: Box::new(step),
                recorded_at_ns: at,
            })
            .expect("step");
    }
    let outcome = campaign
        .apply(&CampaignCommand::Finalize {
            command_id: CampaignCommandId(bytes(40)),
            campaign_id: bytes(34),
            bundle_id: bytes(41),
            evaluated_at_ns: end,
            recorded_at_ns: end,
        })
        .expect("finalize");
    let CampaignDetail::Finalized(bundle) = outcome.detail else {
        panic!("bundle")
    };
    assert_eq!(bundle.status, CampaignStatus::NotEligible);
    assert!(bundle
        .reasons
        .contains(&EvidenceReason::ScenarioMissing(RequiredScenario::DeadMan)));
    assert!(!bundle.promotion_authority_granted);
}

#[test]
fn evidence_file_is_create_new_checksummed_and_digest_verified() {
    let mut campaign = new_campaign();
    let outcome = run_all(&mut campaign, &complete_campaign_commands());
    let CampaignDetail::Finalized(bundle) = outcome.detail else {
        panic!("bundle")
    };
    let directory = tempdir().expect("dir");
    let path = directory.path().join("evidence.bin");
    write_evidence_bundle_create_new(&path, &bundle).expect("write");
    assert_eq!(&read_evidence_bundle(&path).expect("read"), bundle.as_ref());
    assert!(write_evidence_bundle_create_new(&path, &bundle).is_err());
    let mut bytes = std::fs::read(&path).expect("bytes");
    let index = bytes.len() - 1;
    bytes[index] ^= 1;
    std::fs::write(&path, bytes).expect("corrupt");
    assert!(matches!(
        read_evidence_bundle(&path),
        Err(EvidenceFileError::Checksum)
    ));
}

#[test]
fn schedule_substitution_halts_before_gateway_mutation() {
    let commands = complete_campaign_commands();
    let mut campaign = new_campaign();
    campaign.apply(&commands[0]).expect("register");
    let before = campaign.gateway().snapshot().digest;
    let CampaignCommand::ApplyStep { step, .. } = &commands[1] else {
        panic!("step")
    };
    let mut substituted = step.as_ref().clone();
    substituted.scheduled_at_ns += 1;
    let error = campaign.apply(&CampaignCommand::ApplyStep {
        command_id: CampaignCommandId(bytes(200)),
        campaign_id: bytes(60),
        recorded_at_ns: substituted.scheduled_at_ns,
        step: Box::new(substituted),
    });
    assert_eq!(error, Err(Error::Schedule));
    assert!(campaign.is_halted());
    assert_eq!(campaign.gateway().snapshot().digest, before);
}

#[test]
fn runtime_replay_without_active_session_is_forbidden() {
    let commands = complete_campaign_commands();
    let mut campaign = new_campaign();
    campaign.apply(&commands[0]).expect("register");
    let mut step = CampaignStep {
        sequence: 1,
        scheduled_at_ns: START,
        previous_step_digest: [0; 32],
        action: CampaignAction::Gateway {
            session_id: None,
            command: Box::new(GatewayCommand::ApplyRuntime {
                command_id: GatewayCommandId(bytes(200)),
                command: Box::new(UnifiedCommand::Fund {
                    command_id: UnifiedCommandId(bytes(200)),
                    amount_micros: 1,
                    recorded_at_ns: START,
                }),
                recorded_at_ns: START,
            }),
        },
        step_digest: [0; 32],
    };
    step = step.sealed();
    assert_eq!(
        campaign.apply(&CampaignCommand::ApplyStep {
            command_id: CampaignCommandId(bytes(201)),
            campaign_id: bytes(60),
            step: Box::new(step),
            recorded_at_ns: START,
        }),
        Err(Error::Session)
    );
    assert!(campaign.is_halted());
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
    let commands = complete_campaign_commands();
    let directory = tempdir().expect("dir");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 512 * 1024,
            max_segment_records: 4,
        },
    )
    .expect("writer");
    let recovery = CampaignRecovery {
        campaign: new_campaign(),
        last_sequence: None,
    };
    let mut durable = DurableCampaignRunner::new(writer, recovery).expect("durable");
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let expected = durable.campaign().snapshot().digest;
    let sequence = u64::try_from(commands.len() - 1).expect("sequence");
    let checkpoint = CampaignCheckpoint {
        sequence,
        campaign_digest: expected,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(
        &segments,
        campaign_policy(),
        gateway_config(),
        reconciliation(),
        Some(checkpoint),
    )
    .expect("recover");
    assert_eq!(recovered.campaign.snapshot().digest, expected);

    let recovery = CampaignRecovery {
        campaign: new_campaign(),
        last_sequence: None,
    };
    let mut failing = DurableCampaignRunner::new(FailingJournal::default(), recovery).expect("new");
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(StorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(StorageError::Halted(_))
    ));
    assert_eq!(failing.campaign().snapshot().accepted_commands, 0);
}

#[test]
fn identical_multi_day_campaigns_have_identical_digests() {
    let commands = complete_campaign_commands();
    let mut first = new_campaign();
    let mut second = new_campaign();
    run_all(&mut first, &commands);
    run_all(&mut second, &commands);
    assert_eq!(first.snapshot().digest, second.snapshot().digest);
    assert_eq!(first.snapshot().last_bundle, second.snapshot().last_bundle);
}

proptest! {
    #[test]
    fn overlapping_sessions_never_validate(overlap in 1_i64..DAY) {
        let sessions = vec![
            RecordedSession {
                session_id: bytes(1),
                start_ns: START,
                end_ns: START + DAY,
                recording_digest: bytes(2),
            },
            RecordedSession {
                session_id: bytes(3),
                start_ns: START + DAY - overlap,
                end_ns: START + 2 * DAY,
                recording_digest: bytes(4),
            },
        ];
        let manifest = CampaignManifest {
            campaign_id: bytes(5),
            start_ns: START,
            end_ns: START + 2 * DAY,
            sessions,
            required_scenarios: vec![RequiredScenario::DeadMan],
            expected_step_count: 1,
            expected_schedule_digest: bytes(6),
            manifest_digest: [0; 32],
        }.sealed();
        prop_assert_eq!(validate_manifest(&manifest, &campaign_policy()), Err(Error::Manifest));
    }
}
