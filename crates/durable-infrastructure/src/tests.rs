use super::*;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> InfrastructurePolicy {
    InfrastructurePolicy {
        maximum_provider_report_age_ns: 10_000,
        maximum_plan_lifetime_ns: 5_000,
        maximum_observation_age_ns: 500,
        maximum_backoff_ns: 100,
        maximum_batch_bytes: 1_000_000,
        maximum_schema_epoch: 8,
    }
}

fn production_config() -> ProductionInfrastructureConfig {
    ProductionInfrastructureConfig {
        config_id: id(200),
        environment: "production".to_owned(),
        region: "eligible-1".to_owned(),
        endpoints: vec![
            BackendEndpoint {
                backend: BackendKind::PostgreSql,
                uri: "postgresql+tls://db.internal:5432/ledger".to_owned(),
                credential_ref: Some("vault/prod/postgres".to_owned()),
            },
            BackendEndpoint {
                backend: BackendKind::Redpanda,
                uri: "kafka+tls://events.internal:9093".to_owned(),
                credential_ref: Some("vault/prod/redpanda".to_owned()),
            },
            BackendEndpoint {
                backend: BackendKind::ClickHouse,
                uri: "https://analytics.internal:8443".to_owned(),
                credential_ref: Some("vault/prod/clickhouse".to_owned()),
            },
            BackendEndpoint {
                backend: BackendKind::ParquetArchive,
                uri: "s3://archive-bucket/market-events".to_owned(),
                credential_ref: Some("vault/prod/archive".to_owned()),
            },
        ],
        maximum_connections_per_backend: 32,
        request_timeout_ns: 2_000_000_000,
        archive_retention_days: 365,
        read_only: true,
        order_submission_enabled: false,
        config_digest: [0; 32],
    }
    .sealed()
}

#[test]
fn production_config_validates_without_network_access() {
    let config = production_config();
    assert!(config.validate().is_ok());
    assert!(config.verify_digest());
}

#[test]
fn production_config_rejects_embedded_credentials_and_authority() {
    let mut config = production_config();
    config.endpoints[0].uri = "postgresql+tls://user:password@db.internal/ledger".to_owned();
    config = config.sealed();
    assert_eq!(
        config.validate(),
        Err(ProductionConfigError::EmbeddedCredential)
    );

    let mut config = production_config();
    config.order_submission_enabled = true;
    config = config.sealed();
    assert_eq!(
        config.validate(),
        Err(ProductionConfigError::Field(
            "read_only/order_submission_enabled"
        ))
    );
}

#[test]
fn production_config_rejects_wrong_scheme_and_duplicate_backend() {
    let mut config = production_config();
    config.endpoints[2].uri = "http://analytics.internal:8123".to_owned();
    config = config.sealed();
    assert_eq!(
        config.validate(),
        Err(ProductionConfigError::EndpointScheme {
            backend: BackendKind::ClickHouse
        })
    );

    let mut config = production_config();
    config.endpoints[1].backend = BackendKind::PostgreSql;
    config = config.sealed();
    assert_eq!(config.validate(), Err(ProductionConfigError::EndpointSet));
}

fn upstream() -> ProviderCertificationReport {
    ProviderCertificationReport {
        report_id: id(1),
        plan_digest: id(2),
        session_report_digest: id(3),
        contract_digest: id(4),
        covered_scenarios: ProviderScenario::required(),
        final_epoch: 3,
        finalized_at_ns: 100,
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
    .sealed()
}

fn authority(backend: BackendKind) -> AuthorityClass {
    match backend {
        BackendKind::PostgreSql => AuthorityClass::AuthoritativeLedgerProjection,
        BackendKind::Redpanda => AuthorityClass::OrderedEventDistribution,
        BackendKind::ClickHouse => AuthorityClass::DerivedAnalytics,
        BackendKind::ParquetArchive => AuthorityClass::ImmutableReplayArchive,
    }
}

fn backend_byte(backend: BackendKind) -> u8 {
    match backend {
        BackendKind::PostgreSql => 10,
        BackendKind::Redpanda => 20,
        BackendKind::ClickHouse => 30,
        BackendKind::ParquetArchive => 40,
    }
}

fn contract(backend: BackendKind) -> BackendContract {
    let base = backend_byte(backend);
    BackendContract {
        backend,
        authority: authority(backend),
        cluster_digest: id(base),
        region_digest: id(base + 1),
        namespace_digest: id(base + 2),
        schema_digest: id(base + 3),
        initial_schema_epoch: 1,
        maximum_batch_bytes: 100_000,
        tls_required: true,
        public_administration_allowed: false,
        credential_embedded: false,
        external_connection_enabled: false,
        financial_fact_origination_allowed: false,
        contract_digest: [0; 32],
    }
    .sealed()
}

fn plan() -> InfrastructurePlan {
    InfrastructurePlan {
        plan_id: id(50),
        provider_report: upstream(),
        contracts: BackendKind::ALL.into_iter().map(contract).collect(),
        required_scenarios: InfrastructureScenario::ALL.to_vec(),
        created_at_ns: 200,
        expires_at_ns: 4_000,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn record(backend: BackendKind, at: i64) -> DurableRecord {
    let base = backend_byte(backend);
    DurableRecord {
        record_id: id(base + 4),
        idempotency_digest: id(base + 5),
        backend,
        sequence: 0,
        event_time_ns: at - 1,
        received_time_ns: at,
        payload_digest: id(base + 6),
        previous_record_digest: [0; 32],
        byte_length: 64,
        record_digest: [0; 32],
    }
    .sealed()
}

fn disposition(scenario: InfrastructureScenario) -> InfrastructureDisposition {
    match scenario {
        InfrastructureScenario::Commit => InfrastructureDisposition::Commit,
        InfrastructureScenario::IdempotentReplay => InfrastructureDisposition::NoOp,
        InfrastructureScenario::IdempotencyConflict
        | InfrastructureScenario::SequenceGap
        | InfrastructureScenario::Corruption => InfrastructureDisposition::Halt,
        InfrastructureScenario::Backpressure => InfrastructureDisposition::Backoff,
        InfrastructureScenario::MigrationForward => InfrastructureDisposition::Migrate,
        InfrastructureScenario::MigrationRollback => InfrastructureDisposition::Rollback,
        InfrastructureScenario::SnapshotRestore => InfrastructureDisposition::Restore,
        InfrastructureScenario::ReplayConvergence => InfrastructureDisposition::Converged,
    }
}

fn observation(
    backend: BackendKind,
    scenario: InfrastructureScenario,
    identity: u8,
    at: i64,
) -> InfrastructureObservation {
    let base_contract = contract(backend);
    let stored = record(backend, 210 + i64::from(backend_byte(backend)));
    let new_schema = id(backend_byte(backend) + 7);
    let (prior_schema, resulting_schema, epoch) = match scenario {
        InfrastructureScenario::MigrationForward => (base_contract.schema_digest, new_schema, 2),
        InfrastructureScenario::MigrationRollback => (new_schema, base_contract.schema_digest, 2),
        _ => ([0; 32], [0; 32], 0),
    };
    let convergence = matches!(
        scenario,
        InfrastructureScenario::SnapshotRestore | InfrastructureScenario::ReplayConvergence
    );
    InfrastructureObservation {
        observation_id: id(identity),
        backend,
        scenario,
        disposition: disposition(scenario),
        contract_digest: base_contract.contract_digest,
        record: if matches!(
            scenario,
            InfrastructureScenario::Commit | InfrastructureScenario::IdempotentReplay
        ) {
            Some(stored)
        } else {
            None
        },
        prior_schema_digest: prior_schema,
        resulting_schema_digest: resulting_schema,
        schema_epoch: epoch,
        manifest_digest: if convergence {
            id(identity + 1)
        } else {
            [0; 32]
        },
        expected_state_digest: if convergence {
            id(identity + 2)
        } else {
            [0; 32]
        },
        observed_state_digest: if convergence {
            id(identity + 2)
        } else {
            [0; 32]
        },
        backoff_ns: if scenario == InfrastructureScenario::Backpressure {
            50
        } else {
            0
        },
        observed_at_ns: at,
        isolated_fixture: matches!(
            scenario,
            InfrastructureScenario::IdempotencyConflict
                | InfrastructureScenario::SequenceGap
                | InfrastructureScenario::Corruption
        ),
        record_dropped: false,
        automatic_retry_attempted: false,
        credential_loaded: false,
        socket_opened: false,
        external_mutation_observed: false,
        financial_authority_granted: false,
        observation_digest: [0; 32],
    }
    .sealed()
}

fn registered() -> DurableInfrastructureCertification {
    let mut owner = DurableInfrastructureCertification::new(policy()).unwrap();
    owner
        .apply(&InfrastructureCommand::Register {
            command_id: InfrastructureCommandId(id(60)),
            plan: Box::new(plan()),
            recorded_at_ns: 200,
        })
        .unwrap();
    owner
}

#[test]
fn complete_cartesian_campaign_certifies_without_external_authority() {
    let mut owner = registered();
    let mut command_identity = 70_u8;
    let mut at = 220_i64;
    for backend in BackendKind::ALL {
        for scenario in InfrastructureScenario::ALL {
            let value = observation(backend, scenario, command_identity.wrapping_add(1), at);
            owner
                .apply(&InfrastructureCommand::Observe {
                    command_id: InfrastructureCommandId(id(command_identity)),
                    observation: Box::new(value),
                    recorded_at_ns: at,
                })
                .unwrap();
            command_identity = command_identity.wrapping_add(2);
            at += 1;
        }
    }
    let outcome = owner
        .apply(&InfrastructureCommand::Finalize {
            command_id: InfrastructureCommandId(id(200)),
            report_id: id(201),
            finalized_at_ns: at,
            recorded_at_ns: at,
        })
        .unwrap();
    let report = match outcome.detail {
        InfrastructureDetail::Finalized(value) => *value,
        _ => panic!("report"),
    };
    assert!(report.verify_digest());
    assert_eq!(report.covered_matrix.len(), 40);
    assert!(!report.external_environment_certified && !report.credential_material_created);
    assert!(
        !report.socket_opened
            && !report.external_mutation_observed
            && !report.financial_authority_granted
    );
    assert!(
        !report.deployment_authority_granted
            && !report.trading_authority_granted
            && !report.submission_authority_granted
    );
}

#[test]
fn derived_backend_cannot_claim_financial_authority() {
    let mut bad = plan();
    bad.contracts
        .iter_mut()
        .find(|item| item.backend == BackendKind::ClickHouse)
        .unwrap()
        .financial_fact_origination_allowed = true;
    let index = bad
        .contracts
        .iter()
        .position(|item| item.backend == BackendKind::ClickHouse)
        .unwrap();
    bad.contracts[index] = bad.contracts[index].clone().sealed();
    bad = bad.sealed(&policy());
    let mut owner = DurableInfrastructureCertification::new(policy()).unwrap();
    assert_eq!(
        owner
            .apply(&InfrastructureCommand::Register {
                command_id: InfrastructureCommandId(id(202)),
                plan: Box::new(bad),
                recorded_at_ns: 200
            })
            .unwrap_err(),
        Error::Plan
    );
    assert!(owner.is_halted());
}

#[test]
fn backpressure_drop_and_migration_substitution_halt() {
    let mut owner = registered();
    let mut dropped = observation(
        BackendKind::Redpanda,
        InfrastructureScenario::Backpressure,
        203,
        220,
    );
    dropped.record_dropped = true;
    dropped = dropped.sealed();
    assert_eq!(
        owner
            .apply(&InfrastructureCommand::Observe {
                command_id: InfrastructureCommandId(id(204)),
                observation: Box::new(dropped),
                recorded_at_ns: 220
            })
            .unwrap_err(),
        Error::Transition
    );
    assert!(owner.is_halted());

    let mut migration_owner = registered();
    let mut substituted = observation(
        BackendKind::PostgreSql,
        InfrastructureScenario::MigrationForward,
        205,
        220,
    );
    substituted.prior_schema_digest = id(250);
    substituted = substituted.sealed();
    assert_eq!(
        migration_owner
            .apply(&InfrastructureCommand::Observe {
                command_id: InfrastructureCommandId(id(206)),
                observation: Box::new(substituted),
                recorded_at_ns: 220
            })
            .unwrap_err(),
        Error::Transition
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
fn journal_checkpoint_report_and_sync_failure_are_fail_closed() {
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
    let recovery = InfrastructureRecovery {
        owner: DurableInfrastructureCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableInfrastructureOwner::new(writer, recovery).unwrap();
    let command = InfrastructureCommand::Register {
        command_id: InfrastructureCommandId(id(210)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    };
    durable.apply(&command).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let checkpoint = InfrastructureCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let checkpoint_path = dir.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&checkpoint_path).unwrap(), checkpoint);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).unwrap();
    assert_eq!(recovered.owner.snapshot().digest, expected);

    let report = InfrastructureReport {
        report_id: id(211),
        plan_digest: id(212),
        provider_report_digest: id(213),
        covered_matrix: Vec::new(),
        terminal_state_digest: id(214),
        finalized_at_ns: 300,
        status: InfrastructureReportStatus::LocallyCertified,
        external_environment_certified: false,
        credential_material_created: false,
        socket_opened: false,
        external_mutation_observed: false,
        financial_authority_granted: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        submission_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed();
    let report_path = dir.path().join("report.bin");
    write_report_create_new(&report_path, &report).unwrap();
    assert_eq!(read_report(&report_path).unwrap(), report);
    assert!(write_report_create_new(&report_path, &report).is_err());

    let recovery = InfrastructureRecovery {
        owner: DurableInfrastructureCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableInfrastructureOwner::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&command),
        Err(InfrastructureStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&command),
        Err(InfrastructureStorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn oversized_record_never_commits(extra in 1_u64..1_000_000) {
        let mut owner = registered();
        let mut value = observation(BackendKind::PostgreSql, InfrastructureScenario::Commit, 207, 220);
        let mut oversized = value.record.take().unwrap(); oversized.byte_length = contract(BackendKind::PostgreSql).maximum_batch_bytes + extra; oversized = oversized.sealed(); value.record = Some(oversized); value = value.sealed();
        let result = owner.apply(&InfrastructureCommand::Observe { command_id: InfrastructureCommandId(id(208)), observation: Box::new(value), recorded_at_ns: 220 });
        prop_assert!(result.is_err());
        prop_assert!(owner.snapshot().covered_matrix.is_empty());
    }
}
