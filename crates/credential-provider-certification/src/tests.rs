use super::*;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> ProviderPolicy {
    ProviderPolicy {
        maximum_session_report_age_ns: 10_000,
        maximum_plan_lifetime_ns: 5_000,
        maximum_fixture_age_ns: 500,
        maximum_handle_epochs: 16,
        maximum_quota_units: 100,
    }
}

fn upstream() -> ShadowSessionReport {
    ShadowSessionReport {
        report_id: id(1),
        plan_digest: id(2),
        gateway_report_digest: id(3),
        final_attestation_digest: id(4),
        covered_scenarios: SessionScenario::ALL.to_vec(),
        opened_lease_count: 4,
        rotation_count: 1,
        recovery_count: 3,
        finalized_at_ns: 100,
        status: SessionReportStatus::SimulationCompleted,
        credential_material_created: false,
        signature_produced: false,
        provider_contacted: false,
        socket_opened: false,
        authentication_authority_granted: false,
        external_submission_authority_granted: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed()
}

fn contract() -> ProviderContract {
    ProviderContract {
        provider_id_digest: id(10),
        tenant_digest: id(11),
        primary_region_digest: id(12),
        recovery_region_digest: id(13),
        key_purpose_digest: id(14),
        algorithm: ProviderAlgorithm::Ed25519,
        maximum_quota_units: 50,
        valid_from_ns: 100,
        valid_until_ns: 5_000,
        key_material_embedded: false,
        provider_credential_embedded: false,
        export_allowed: false,
        signing_allowed: false,
        external_mutation_allowed: false,
        contract_digest: [0; 32],
    }
    .sealed()
}

fn plan() -> ProviderCertificationPlan {
    ProviderCertificationPlan {
        plan_id: id(20),
        session_report: upstream(),
        contract: contract(),
        required_scenarios: ProviderScenario::required(),
        created_at_ns: 200,
        expires_at_ns: 4_000,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn handle(identity: u8, epoch: u64, predecessor: [u8; 32], at: i64) -> OpaqueProviderHandle {
    OpaqueProviderHandle {
        handle_id_digest: id(identity),
        contract_digest: contract().contract_digest,
        predecessor_handle_digest: predecessor,
        epoch,
        attestation_digest: id(identity + 1),
        region_digest: contract().primary_region_digest,
        observed_at_ns: at,
        key_material_present: false,
        credential_material_present: false,
        signature_bytes_present: false,
        provider_contacted: false,
        handle_digest: [0; 32],
    }
    .sealed()
}

fn fixture(
    identity: u8,
    scenario: ProviderScenario,
    disposition: ProviderDisposition,
    handle: Option<OpaqueProviderHandle>,
    at: i64,
) -> RecordedProviderFixture {
    RecordedProviderFixture {
        fixture_id: id(identity),
        scenario,
        disposition,
        handle,
        subject_digest: contract().contract_digest,
        observed_at_ns: at,
        quota_units: if scenario == ProviderScenario::QuotaExceeded {
            51
        } else {
            1
        },
        retry_attempted: false,
        key_material_observed: false,
        credential_material_observed: false,
        signature_produced: false,
        provider_contacted: false,
        socket_opened: false,
        external_mutation_observed: false,
        fixture_digest: [0; 32],
    }
    .sealed()
}

fn apply_fixture(
    owner: &mut CredentialProviderCertification,
    value: RecordedProviderFixture,
    command: u8,
) -> ProviderOutcome {
    let at = value.observed_at_ns;
    owner
        .apply(&ProviderCommand::RecordFixture {
            command_id: ProviderCommandId(id(command)),
            fixture: Box::new(value),
            recorded_at_ns: at,
        })
        .expect("fixture")
}

fn registered() -> CredentialProviderCertification {
    let mut owner = CredentialProviderCertification::new(policy()).expect("policy");
    owner
        .apply(&ProviderCommand::Register {
            command_id: ProviderCommandId(id(30)),
            plan: Box::new(plan()),
            recorded_at_ns: 200,
        })
        .expect("register");
    owner
}

#[test]
#[allow(clippy::too_many_lines)]
fn complete_campaign_certifies_without_authority() {
    let mut owner = registered();
    let first = handle(40, 1, [0; 32], 210);
    apply_fixture(
        &mut owner,
        fixture(
            41,
            ProviderScenario::AcquisitionSuccess,
            ProviderDisposition::Accept,
            Some(first.clone()),
            210,
        ),
        61,
    );
    let second = handle(42, 2, first.handle_digest, 220);
    apply_fixture(
        &mut owner,
        fixture(
            43,
            ProviderScenario::RotationSuccess,
            ProviderDisposition::Accept,
            Some(second),
            220,
        ),
        62,
    );
    apply_fixture(
        &mut owner,
        fixture(
            44,
            ProviderScenario::QuotaExceeded,
            ProviderDisposition::Backoff,
            None,
            230,
        ),
        63,
    );
    apply_fixture(
        &mut owner,
        fixture(
            45,
            ProviderScenario::ProviderOutage,
            ProviderDisposition::Backoff,
            None,
            240,
        ),
        64,
    );
    apply_fixture(
        &mut owner,
        fixture(
            46,
            ProviderScenario::AttestationMismatch,
            ProviderDisposition::Deny,
            None,
            250,
        ),
        65,
    );
    apply_fixture(
        &mut owner,
        fixture(
            47,
            ProviderScenario::StaleEpoch,
            ProviderDisposition::Deny,
            None,
            260,
        ),
        66,
    );
    let split = apply_fixture(
        &mut owner,
        fixture(
            48,
            ProviderScenario::SplitBrainAttempt,
            ProviderDisposition::Reconcile,
            None,
            270,
        ),
        67,
    );
    let requirement = match split.detail {
        ProviderDetail::RecoveryRequired(value) => *value,
        _ => panic!("recovery"),
    };
    let evidence = RecordedProviderRecovery {
        recovery_id: id(49),
        requirement_digest: requirement.requirement_digest,
        recovered_epoch: requirement.required_epoch,
        state_digest: id(50),
        destination_region_digest: contract().recovery_region_digest,
        observed_at_ns: 280,
        no_mutation_observed: true,
        key_material_observed: false,
        credential_material_observed: false,
        signature_produced: false,
        provider_contacted: false,
        socket_opened: false,
        handle_activated: false,
        recovery_digest: [0; 32],
    }
    .sealed();
    owner
        .apply(&ProviderCommand::Recover {
            command_id: ProviderCommandId(id(68)),
            requirement: Box::new(requirement),
            evidence,
            recorded_at_ns: 280,
        })
        .expect("recover");
    apply_fixture(
        &mut owner,
        fixture(
            51,
            ProviderScenario::DisasterRecovery,
            ProviderDisposition::ManualRecovery,
            None,
            290,
        ),
        69,
    );
    let third = handle(52, 3, [0; 32], 300);
    apply_fixture(
        &mut owner,
        fixture(
            53,
            ProviderScenario::AcquisitionSuccess,
            ProviderDisposition::Accept,
            Some(third.clone()),
            300,
        ),
        70,
    );
    apply_fixture(
        &mut owner,
        fixture(
            54,
            ProviderScenario::RevocationSuccess,
            ProviderDisposition::Revoke,
            Some(third),
            310,
        ),
        71,
    );
    let outcome = owner
        .apply(&ProviderCommand::Finalize {
            command_id: ProviderCommandId(id(72)),
            report_id: id(55),
            finalized_at_ns: 320,
            recorded_at_ns: 320,
        })
        .expect("finalize");
    let report = match outcome.detail {
        ProviderDetail::Finalized(value) => *value,
        _ => panic!("report"),
    };
    assert!(report.verify_digest());
    assert_eq!(report.covered_scenarios, ProviderScenario::required());
    assert!(!report.key_material_created && !report.provider_credential_created);
    assert!(!report.signature_produced && !report.provider_contacted && !report.socket_opened);
    assert!(!report.external_mutation_observed && !report.signing_authority_granted);
    assert!(
        !report.submission_authority_granted
            && !report.deployment_authority_granted
            && !report.trading_authority_granted
    );
}

#[test]
fn authority_bearing_upstream_halts() {
    let mut bad = plan();
    bad.session_report.trading_authority_granted = true;
    bad.session_report = bad.session_report.sealed();
    bad = bad.sealed(&policy());
    let mut owner = CredentialProviderCertification::new(policy()).expect("policy");
    assert_eq!(
        owner
            .apply(&ProviderCommand::Register {
                command_id: ProviderCommandId(id(80)),
                plan: Box::new(bad),
                recorded_at_ns: 200
            })
            .expect_err("deny"),
        Error::Upstream
    );
    assert!(owner.is_halted());
}

#[test]
fn rotation_substitution_and_automatic_retry_halt() {
    let mut owner = registered();
    let first = handle(81, 1, [0; 32], 210);
    apply_fixture(
        &mut owner,
        fixture(
            82,
            ProviderScenario::AcquisitionSuccess,
            ProviderDisposition::Accept,
            Some(first),
            210,
        ),
        83,
    );
    let bad = handle(84, 2, id(99), 220);
    assert_eq!(
        owner
            .apply(&ProviderCommand::RecordFixture {
                command_id: ProviderCommandId(id(85)),
                fixture: Box::new(fixture(
                    86,
                    ProviderScenario::RotationSuccess,
                    ProviderDisposition::Accept,
                    Some(bad),
                    220
                )),
                recorded_at_ns: 220
            })
            .expect_err("deny"),
        Error::Lifecycle
    );
    assert!(owner.is_halted());

    let mut retry_owner = registered();
    let mut outage = fixture(
        87,
        ProviderScenario::ProviderOutage,
        ProviderDisposition::Backoff,
        None,
        210,
    );
    outage.retry_attempted = true;
    outage = outage.sealed();
    assert_eq!(
        retry_owner
            .apply(&ProviderCommand::RecordFixture {
                command_id: ProviderCommandId(id(88)),
                fixture: Box::new(outage),
                recorded_at_ns: 210
            })
            .expect_err("deny"),
        Error::Lifecycle
    );
}

#[derive(Debug, Default)]
struct FailingJournal {
    last: Option<u64>,
}

impl EventJournal for FailingJournal {
    fn append_event(
        &mut self,
        envelope: &event_schema::EventEnvelope,
    ) -> Result<u64, JournalBackendError> {
        self.last = Some(envelope.sequence);
        Ok(0)
    }

    fn sync_events(&self) -> Result<(), JournalBackendError> {
        Err(JournalBackendError::Single(JournalError::Io(
            std::io::Error::other("sync failure"),
        )))
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}

#[test]
fn durable_replay_checkpoint_report_and_sync_failure_are_fail_closed() {
    let dir = tempdir().unwrap();
    let segments = dir.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 4 * 1024 * 1024,
            max_segment_records: 2,
        },
    )
    .unwrap();
    let recovery = ProviderRecovery {
        owner: CredentialProviderCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableCredentialProviderCertification::new(writer, recovery).unwrap();
    let command = ProviderCommand::Register {
        command_id: ProviderCommandId(id(100)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    };
    durable.apply(&command).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let checkpoint = ProviderCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let checkpoint_path = dir.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&checkpoint_path).unwrap(), checkpoint);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).unwrap();
    assert_eq!(recovered.owner.snapshot().digest, expected);

    let report = ProviderCertificationReport {
        report_id: id(101),
        plan_digest: id(102),
        session_report_digest: id(103),
        contract_digest: id(104),
        covered_scenarios: ProviderScenario::required(),
        final_epoch: 3,
        finalized_at_ns: 400,
        status: ProviderReportStatus::OfflineCertified,
        key_material_created: false,
        provider_credential_created: false,
        signature_produced: false,
        provider_contacted: false,
        socket_opened: false,
        external_mutation_observed: false,
        signing_authority_granted: false,
        submission_authority_granted: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed();
    let report_path = dir.path().join("report.bin");
    write_report_create_new(&report_path, &report).unwrap();
    assert_eq!(read_report(&report_path).unwrap(), report);
    assert!(write_report_create_new(&report_path, &report).is_err());

    let recovery = ProviderRecovery {
        owner: CredentialProviderCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing =
        DurableCredentialProviderCertification::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&command),
        Err(ProviderStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&command),
        Err(ProviderStorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn handle_epoch_above_policy_never_activates(epoch in 17_u64..u64::MAX) {
        let mut owner = registered();
        let excessive = handle(90, epoch, [0; 32], 210);
        let result = owner.apply(&ProviderCommand::RecordFixture { command_id: ProviderCommandId(id(91)), fixture: Box::new(fixture(92, ProviderScenario::AcquisitionSuccess, ProviderDisposition::Accept, Some(excessive), 210)), recorded_at_ns: 210 });
        prop_assert!(result.is_err());
        prop_assert!(owner.snapshot().current_handle.is_none());
    }
}
