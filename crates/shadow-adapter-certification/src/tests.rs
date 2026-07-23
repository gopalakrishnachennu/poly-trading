use super::*;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use tempfile::tempdir;

const BASE: i64 = 1_000_000;

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
        required_regions: vec!["primary-us".into(), "failover-eu".into()],
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

fn policy(valid_from: i64, valid_until: i64) -> SignerPolicyFrame {
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
        valid_from_ns: valid_from,
        valid_until_ns: valid_until,
    }
}

fn intent(at: i64) -> DryRunIntent {
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

#[allow(clippy::too_many_arguments)]
fn complete_commands(
    allowance: i128,
    gas: i128,
    relayer_available: bool,
    queue_depth: u64,
    blocked_region: Option<&str>,
    profile: u8,
) -> Vec<CertificationCommand> {
    let contract = contract();
    let mut commands = vec![CertificationCommand::RegisterContract {
        command_id: CertificationCommandId(bytes(1)),
        contract: contract.clone(),
        recorded_at_ns: BASE,
    }];
    for (index, kind) in FixtureKind::ALL.into_iter().enumerate() {
        let number = u8::try_from(index + 2).expect("id");
        commands.push(CertificationCommand::RecordFixture {
            command_id: CertificationCommandId(bytes(number)),
            fixture: RecordedFixture {
                fixture_id: FixtureId(bytes(number)),
                contract_digest: contract.contract_digest,
                sequence: u64::try_from(index + 1).expect("sequence"),
                kind,
                captured_at_ns: BASE + i64::from(number),
                received_at_ns: BASE + i64::from(number),
                payload_digest: bytes(number.saturating_add(40)),
            },
            recorded_at_ns: BASE + i64::from(number),
        });
    }
    for (offset, region) in contract.required_regions.iter().enumerate() {
        let number = u8::try_from(11 + offset).expect("id");
        commands.push(CertificationCommand::ObserveEligibility {
            command_id: CertificationCommandId(bytes(number)),
            attestation: EligibilityAttestation {
                sequence: 1,
                region: region.clone(),
                egress_fingerprint: bytes(number),
                eligible: blocked_region != Some(region.as_str()),
                checked_at_ns: BASE + 20,
                valid_until_ns: BASE + 100,
                source_digest: bytes(number.saturating_add(50)),
            },
            recorded_at_ns: BASE + 20,
        });
    }
    commands.push(CertificationCommand::ObserveOperational {
        command_id: CertificationCommandId(bytes(13)),
        observation: OperationalObservation {
            sequence: 1,
            wallet_alias: "paper-safe".into(),
            chain_id: 137,
            collateral_micros: 3_000_000,
            allowance_micros: allowance,
            gas_micros: gas,
            relayer_available,
            relayer_queue_depth: queue_depth,
            observed_at_ns: BASE + 21,
            valid_until_ns: BASE + 100,
            observation_digest: [0; 32],
        }
        .sealed(),
        recorded_at_ns: BASE + 21,
    });
    let baseline_policy = policy(BASE, BASE + 100);
    let baseline = intent(BASE + 22);
    let mut wrong_contract = baseline.clone();
    wrong_contract.exchange_contract = "wrong-contract".into();
    let mut wrong_token = baseline.clone();
    wrong_token.token = token("other");
    let mut excessive_quantity = baseline.clone();
    excessive_quantity.quantity_micros = 1_000_001;
    let inactive_policy = policy(BASE + 90, BASE + 200);
    for (offset, (dry_policy, dry_intent)) in [
        (baseline_policy.clone(), baseline),
        (baseline_policy.clone(), wrong_contract),
        (baseline_policy.clone(), wrong_token),
        (baseline_policy, excessive_quantity),
        (inactive_policy, intent(BASE + 26)),
    ]
    .into_iter()
    .enumerate()
    {
        let number = u8::try_from(14 + offset).expect("id");
        commands.push(CertificationCommand::DryRunSigner {
            command_id: CertificationCommandId(bytes(number)),
            dry_run_id: DryRunId(bytes(number)),
            policy: dry_policy,
            intent: dry_intent,
            recorded_at_ns: BASE + 22 + i64::try_from(offset).expect("time"),
        });
    }
    for (offset, kind) in FailureKind::ALL.into_iter().enumerate() {
        let number = u8::try_from(19 + offset).expect("id");
        commands.push(CertificationCommand::SimulateFailure {
            command_id: CertificationCommandId(bytes(number)),
            failure_id: FailureId(bytes(number)),
            kind,
            recorded_at_ns: BASE + 30 + i64::try_from(offset).expect("time"),
        });
    }
    commands.push(CertificationCommand::Evaluate {
        command_id: CertificationCommandId(bytes(27)),
        profile_id: bytes(profile),
        evaluated_at_ns: BASE + 50,
        recorded_at_ns: BASE + 50,
    });
    commands
}

fn apply_all(
    authority: &mut ShadowAdapterCertification,
    commands: &[CertificationCommand],
) -> CertificationOutcome {
    let mut last = None;
    for command in commands {
        last = Some(authority.apply(command).expect("command"));
    }
    last.expect("commands")
}

fn report(outcome: &CertificationOutcome) -> &CertificationReport {
    let CertificationDetail::Evaluated(report) = &outcome.detail else {
        panic!("expected report")
    };
    report
}

#[test]
fn complete_evidence_certifies_without_granting_authority() {
    let mut authority = ShadowAdapterCertification::default();
    let outcome = apply_all(
        &mut authority,
        &complete_commands(2_000_000, 100_000, true, 0, None, 90),
    );
    let report = report(&outcome);
    assert_eq!(report.status, CertificationStatus::Certified);
    assert!(report.reasons.is_empty());
    assert!(!report.authority_granted);
    assert!(report.verify_digest());
    assert_eq!(authority.snapshot().fixture_count, FixtureKind::ALL.len());
    assert_eq!(authority.snapshot().dry_run_count, 5);
    assert_eq!(authority.snapshot().failure_count, FailureKind::ALL.len());
}

#[test]
fn fixture_and_failure_classifications_are_conservative() {
    assert_eq!(
        fixture_action(FixtureKind::Restart425),
        FixtureAction::BackoffWithoutAutomaticRetry
    );
    assert_eq!(
        fixture_action(FixtureKind::TakerDelay),
        FixtureAction::RetainBackingUntilDelayEnds
    );
    assert_eq!(
        fixture_action(FixtureKind::UnknownOrder),
        FixtureAction::ReconcileUnknownOrder
    );
    assert_eq!(
        safe_action(FailureKind::UnknownSubmission),
        SafeAction::RetainBackingAndReconcile
    );
    assert_eq!(
        safe_action(FailureKind::SettlementRetrying),
        SafeAction::RetainUnconfirmedValue
    );
    assert!(FailureKind::ALL.into_iter().all(|kind| {
        !matches!(
            safe_action(kind),
            SafeAction::RetainBackingAndReconcile
                if kind != FailureKind::UnknownSubmission
        )
    }));
}

#[test]
fn missing_evidence_is_attributable_not_certified() {
    let mut authority = ShadowAdapterCertification::default();
    authority
        .apply(&CertificationCommand::RegisterContract {
            command_id: CertificationCommandId(bytes(1)),
            contract: contract(),
            recorded_at_ns: BASE,
        })
        .expect("contract");
    let outcome = authority
        .apply(&CertificationCommand::Evaluate {
            command_id: CertificationCommandId(bytes(2)),
            profile_id: bytes(80),
            evaluated_at_ns: BASE + 1,
            recorded_at_ns: BASE + 1,
        })
        .expect("evaluate");
    let report = report(&outcome);
    assert_eq!(report.status, CertificationStatus::NotCertified);
    assert!(report
        .reasons
        .contains(&CertificationReason::BaselineDryRunMissing));
    assert!(report
        .reasons
        .contains(&CertificationReason::OperationalMissing));
    assert!(report
        .reasons
        .contains(&CertificationReason::FixtureMissing(
            FixtureKind::Restart425
        )));
    assert!(report
        .reasons
        .contains(&CertificationReason::FailureSimulationMissing(
            FailureKind::UnknownSubmission
        )));
}

#[test]
fn operational_and_eligibility_failures_deny_independently() {
    for (allowance, gas, relayer, queue, blocked, expected) in [
        (
            1_999_999,
            100_000,
            true,
            0,
            None,
            CertificationReason::AllowanceInsufficient,
        ),
        (
            2_000_000,
            99_999,
            true,
            0,
            None,
            CertificationReason::GasInsufficient,
        ),
        (
            2_000_000,
            100_000,
            false,
            0,
            None,
            CertificationReason::RelayerUnavailable,
        ),
        (
            2_000_000,
            100_000,
            true,
            9,
            None,
            CertificationReason::RelayerQueueExceeded,
        ),
        (
            2_000_000,
            100_000,
            true,
            0,
            Some("failover-eu"),
            CertificationReason::EligibilityBlocked("failover-eu".into()),
        ),
    ] {
        let mut authority = ShadowAdapterCertification::default();
        let outcome = apply_all(
            &mut authority,
            &complete_commands(allowance, gas, relayer, queue, blocked, 91),
        );
        assert_eq!(report(&outcome).status, CertificationStatus::NotCertified);
        assert!(report(&outcome).reasons.contains(&expected));
    }
}

#[test]
fn expired_eligibility_and_operational_evidence_deny_independently() {
    let mut eligibility_commands = complete_commands(2_000_000, 100_000, true, 0, None, 95);
    let eligibility = eligibility_commands
        .iter_mut()
        .find_map(|command| match command {
            CertificationCommand::ObserveEligibility { attestation, .. }
                if attestation.region == "failover-eu" =>
            {
                Some(attestation)
            }
            _ => None,
        })
        .expect("eligibility");
    eligibility.valid_until_ns = BASE + 25;
    let mut authority = ShadowAdapterCertification::default();
    let outcome = apply_all(&mut authority, &eligibility_commands);
    assert!(report(&outcome)
        .reasons
        .contains(&CertificationReason::EligibilityStale("failover-eu".into())));

    let mut operational_commands = complete_commands(2_000_000, 100_000, true, 0, None, 96);
    let operation = operational_commands
        .iter_mut()
        .find_map(|command| match command {
            CertificationCommand::ObserveOperational { observation, .. } => Some(observation),
            _ => None,
        })
        .expect("operation");
    operation.valid_until_ns = BASE + 25;
    *operation = operation.clone().sealed();
    let mut authority = ShadowAdapterCertification::default();
    let outcome = apply_all(&mut authority, &operational_commands);
    assert!(report(&outcome)
        .reasons
        .contains(&CertificationReason::OperationalStale));
}

#[test]
fn signer_dry_runs_cover_permit_and_required_denials() {
    let mut authority = ShadowAdapterCertification::default();
    let commands = complete_commands(2_000_000, 100_000, true, 0, None, 92);
    for command in &commands {
        let outcome = authority.apply(command).expect("command");
        if let CertificationDetail::DryRun(result) = outcome.detail {
            assert!(result.verify_digest());
            assert_eq!(result.permitted, result.reason == DryRunReason::Permitted);
        }
    }
    assert_eq!(
        authority.snapshot().last_report.expect("report").status,
        CertificationStatus::Certified
    );
}

#[test]
fn fixture_sequence_and_wallet_identity_conflicts_halt_absorbingly() {
    let contract = contract();
    let mut fixture_authority = ShadowAdapterCertification::default();
    fixture_authority
        .apply(&CertificationCommand::RegisterContract {
            command_id: CertificationCommandId(bytes(1)),
            contract: contract.clone(),
            recorded_at_ns: BASE,
        })
        .expect("contract");
    fixture_authority
        .apply(&CertificationCommand::RecordFixture {
            command_id: CertificationCommandId(bytes(2)),
            fixture: RecordedFixture {
                fixture_id: FixtureId(bytes(2)),
                contract_digest: contract.contract_digest,
                sequence: 2,
                kind: FixtureKind::Restart425,
                captured_at_ns: BASE + 1,
                received_at_ns: BASE + 1,
                payload_digest: bytes(30),
            },
            recorded_at_ns: BASE + 1,
        })
        .expect("fixture");
    assert_eq!(
        fixture_authority.apply(&CertificationCommand::RecordFixture {
            command_id: CertificationCommandId(bytes(3)),
            fixture: RecordedFixture {
                fixture_id: FixtureId(bytes(3)),
                contract_digest: contract.contract_digest,
                sequence: 1,
                kind: FixtureKind::PostOnlyWindow,
                captured_at_ns: BASE + 2,
                received_at_ns: BASE + 2,
                payload_digest: bytes(31),
            },
            recorded_at_ns: BASE + 2,
        }),
        Err(Error::History)
    );
    assert!(fixture_authority.is_halted());

    let mut wallet_authority = ShadowAdapterCertification::default();
    wallet_authority
        .apply(&CertificationCommand::RegisterContract {
            command_id: CertificationCommandId(bytes(1)),
            contract,
            recorded_at_ns: BASE,
        })
        .expect("contract");
    let observation = |sequence, alias: &str, at| {
        OperationalObservation {
            sequence,
            wallet_alias: alias.into(),
            chain_id: 137,
            collateral_micros: 3_000_000,
            allowance_micros: 2_000_000,
            gas_micros: 100_000,
            relayer_available: true,
            relayer_queue_depth: 0,
            observed_at_ns: at,
            valid_until_ns: at + 100,
            observation_digest: [0; 32],
        }
        .sealed()
    };
    wallet_authority
        .apply(&CertificationCommand::ObserveOperational {
            command_id: CertificationCommandId(bytes(2)),
            observation: observation(1, "paper-safe", BASE + 1),
            recorded_at_ns: BASE + 1,
        })
        .expect("observation");
    assert_eq!(
        wallet_authority.apply(&CertificationCommand::ObserveOperational {
            command_id: CertificationCommandId(bytes(3)),
            observation: observation(2, "substituted-safe", BASE + 2),
            recorded_at_ns: BASE + 2,
        }),
        Err(Error::Identity)
    );
    assert!(wallet_authority.is_halted());
}

#[test]
fn command_idempotency_and_codec_are_content_bound() {
    let command = CertificationCommand::RegisterContract {
        command_id: CertificationCommandId(bytes(1)),
        contract: contract(),
        recorded_at_ns: BASE,
    };
    assert_eq!(
        decode_command(&encode_command(&command).expect("encode")).expect("decode"),
        command
    );
    let mut authority = ShadowAdapterCertification::default();
    let first = authority.apply(&command).expect("first");
    assert_eq!(authority.apply(&command), Ok(first));
    let mut changed = contract();
    changed.rest_host = "https://changed.invalid".into();
    changed = changed.sealed();
    assert_eq!(
        authority.apply(&CertificationCommand::RegisterContract {
            command_id: CertificationCommandId(bytes(1)),
            contract: changed,
            recorded_at_ns: BASE,
        }),
        Err(Error::IdempotencyConflict)
    );
    assert!(authority.is_halted());
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
fn durable_complete_report_replays_and_sync_failure_is_fail_closed() {
    let directory = tempdir().expect("dir");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 256 * 1024,
            max_segment_records: 3,
        },
    )
    .expect("writer");
    let mut durable = DurableCertificationAuthority::new(
        writer,
        CertificationRecovery {
            authority: ShadowAdapterCertification::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    let commands = complete_commands(2_000_000, 100_000, true, 0, None, 93);
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let checkpoint = CertificationCheckpoint {
        sequence: u64::try_from(commands.len() - 1).expect("sequence"),
        authority_digest: durable.authority().snapshot().digest,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, Some(checkpoint)).expect("recover");
    assert_eq!(
        recovered.authority.snapshot().digest,
        checkpoint.authority_digest
    );
    assert_eq!(
        recovered
            .authority
            .snapshot()
            .last_report
            .expect("report")
            .status,
        CertificationStatus::Certified
    );

    let first = &commands[0];
    let mut failing = DurableCertificationAuthority::new(
        FailingJournal::default(),
        CertificationRecovery {
            authority: ShadowAdapterCertification::default(),
            last_sequence: None,
        },
    )
    .expect("failing");
    assert!(matches!(
        failing.apply(first),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(failing.authority().snapshot().accepted_commands, 0);
    assert!(matches!(failing.apply(first), Err(StorageError::Halted(_))));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]
    #[test]
    fn insufficient_allowance_or_gas_never_certifies(
        allowance in 0_i128..2_000_000,
        gas in 0_i128..100_000,
    ) {
        let mut authority = ShadowAdapterCertification::default();
        let outcome = apply_all(
            &mut authority,
            &complete_commands(allowance, gas, true, 0, None, 94),
        );
        prop_assert_eq!(report(&outcome).status, CertificationStatus::NotCertified);
        prop_assert!(!report(&outcome).authority_granted);
    }
}
