use super::*;
use executor_session_simulator::SessionDossierStatus;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(value: u8) -> [u8; 32] {
    [value; 32]
}
fn policy() -> TransportCertificationPolicy {
    TransportCertificationPolicy {
        maximum_dossier_age_ns: 2_000,
        maximum_campaign_age_ns: 2_000,
        maximum_backoff_ns: 500,
        maximum_endpoints: 8,
        maximum_pins: 8,
        maximum_bindings: 8,
    }
}
fn dossier() -> ExecutorSessionDossier {
    ExecutorSessionDossier {
        dossier_id: id(1),
        session_plan_digest: id(2),
        execution_report_digest: id(3),
        isolation_digest: id(4),
        request_template_digests: vec![id(10), id(11)],
        request_chain_digest: id(5),
        reconciliation_chain_digest: id(6),
        resolved_request_count: 2,
        finalized_at_ns: 1_000,
        status: SessionDossierStatus::ProtocolSimulationCompleted,
        simulated_only: true,
        credential_material_created: false,
        signature_authority_granted: false,
        authenticated_transport_granted: false,
        external_submission_authority_granted: false,
        deployment_authority_granted: false,
        dossier_digest: [0; 32],
    }
    .sealed()
}
fn endpoint() -> EndpointPolicy {
    EndpointPolicy {
        hostname: "control.example.internal".into(),
        port: 443,
        server_name: "control.example.internal".into(),
        allowed_paths: vec!["/v1/apply".into(), "/v1/verify".into()],
        certificate_spki_pins: vec![id(20), id(21)],
        resolver_policy_digest: id(22),
        minimum_tls_version: TlsMinimumVersion::Tls13,
        redirects_allowed: false,
        proxy_allowed: false,
        cookies_allowed: false,
        authorization_headers_allowed: false,
        query_credentials_allowed: false,
        wildcard_identity_allowed: false,
        policy_digest: [0; 32],
    }
    .sealed()
}
fn plan() -> TransportCertificationPlan {
    TransportCertificationPlan {
        plan_id: id(30),
        created_at_ns: 1_100,
        expires_at_ns: 2_000,
        session_dossier: dossier(),
        endpoint_policy: endpoint(),
        request_bindings: vec![
            CanonicalRequestBinding {
                sequence: 0,
                template_digest: id(10),
                method: HttpMethod::Post,
                path: "/v1/apply".into(),
                body_digest: id(31),
                canonical_bytes_digest: id(32),
                binding_digest: [0; 32],
            }
            .sealed(),
            CanonicalRequestBinding {
                sequence: 1,
                template_digest: id(11),
                method: HttpMethod::Get,
                path: "/v1/verify".into(),
                body_digest: id(33),
                canonical_bytes_digest: id(34),
                binding_digest: [0; 32],
            }
            .sealed(),
        ],
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}
fn register() -> TransportCommand {
    TransportCommand::Register {
        command_id: TransportCommandId(id(40)),
        plan: Box::new(plan()),
        recorded_at_ns: 1_100,
    }
}

fn fixture(index: usize) -> RecordedTransportFixture {
    let case = TransportFixtureCase::ALL[index];
    let sequence = u8::try_from(index).unwrap();
    let mut value = RecordedTransportFixture {
        sequence,
        case,
        expected: case.expected(),
        observed: case.expected(),
        observed_at_ns: 1_110 + i64::try_from(index).unwrap(),
        hostname: "control.example.internal".into(),
        resolver_answer_digest: id(50),
        server_name: "control.example.internal".into(),
        presented_spki_digest: id(20),
        path: "/v1/apply".into(),
        serialized_request_digest: id(32),
        status_code: None,
        backoff_ns: None,
        ambiguity_digest: None,
        reconciliation_digest: None,
        recorded_fixture: true,
        socket_opened: false,
        credential_loaded: false,
        signature_produced: false,
        authenticated_request_sent: false,
        external_submission_observed: false,
        external_mutation_observed: false,
        fixture_source_digest: id(60 + sequence),
        fixture_digest: [0; 32],
    };
    match case {
        TransportFixtureCase::DnsWrongHost => value.hostname = "evil.example.invalid".into(),
        TransportFixtureCase::TlsWrongPin => value.presented_spki_digest = id(99),
        TransportFixtureCase::EndpointForbidden => value.path = "/admin".into(),
        TransportFixtureCase::NoncanonicalRequest => value.serialized_request_digest = id(98),
        TransportFixtureCase::Timeout => value.backoff_ns = Some(100),
        TransportFixtureCase::RateLimited => {
            value.status_code = Some(429);
            value.backoff_ns = Some(200);
        }
        TransportFixtureCase::UnknownResponse => {
            value.ambiguity_digest = Some(id(96));
        }
        TransportFixtureCase::NoMutationReconciliation => {
            value.ambiguity_digest = Some(id(96));
            value.reconciliation_digest = Some(id(97));
        }
        _ => {}
    }
    value.sealed()
}

fn run_complete() -> TransportAdapterCertification {
    let mut owner = TransportAdapterCertification::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    for index in 0..TransportFixtureCase::ALL.len() {
        let evidence = fixture(index);
        owner
            .apply(&TransportCommand::RecordFixture {
                command_id: TransportCommandId(id(70 + u8::try_from(index).unwrap())),
                recorded_at_ns: evidence.observed_at_ns,
                fixture: Box::new(evidence),
            })
            .unwrap();
    }
    owner
}

#[test]
fn complete_recorded_matrix_certifies_without_transport_authority() {
    let mut owner = run_complete();
    let certificate = match owner
        .apply(&TransportCommand::Finalize {
            command_id: TransportCommandId(id(90)),
            certificate_id: id(91),
            certified_at_ns: 1_130,
            recorded_at_ns: 1_130,
        })
        .unwrap()
        .detail
    {
        TransportDetail::Finalized(value) => *value,
        _ => panic!("certificate"),
    };
    assert!(certificate.verify_digest());
    assert_eq!(certificate.fixture_count, 12);
    assert!(certificate.recorded_fixtures_only);
    assert!(!certificate.socket_authority_granted);
    assert!(!certificate.external_submission_authority_granted);
    let dir = tempdir().unwrap();
    let path = dir.path().join("certificate.bin");
    write_certificate_create_new(&path, &certificate).unwrap();
    assert_eq!(read_certificate(&path).unwrap(), certificate);
    assert!(write_certificate_create_new(&path, &certificate).is_err());
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        read_certificate(&path),
        Err(TransportCertificateFileError::Checksum)
    ));
}

#[test]
fn stale_substituted_or_authority_bearing_dossier_halts() {
    let mut bad = plan();
    bad.session_dossier.external_submission_authority_granted = true;
    bad.session_dossier = bad.session_dossier.sealed();
    bad = bad.sealed(&policy());
    let mut owner = TransportAdapterCertification::new(policy()).unwrap();
    assert_eq!(
        owner.apply(&TransportCommand::Register {
            command_id: TransportCommandId(id(1)),
            plan: Box::new(bad),
            recorded_at_ns: 1_100
        }),
        Err(Error::Plan)
    );
    assert!(owner.is_halted());
    let mut bad = plan();
    bad.request_bindings[0].template_digest = id(88);
    bad.request_bindings[0] = bad.request_bindings[0].clone().sealed();
    bad = bad.sealed(&policy());
    let mut owner = TransportAdapterCertification::new(policy()).unwrap();
    assert!(matches!(
        owner.apply(&TransportCommand::Register {
            command_id: TransportCommandId(id(1)),
            plan: Box::new(bad),
            recorded_at_ns: 1_100
        }),
        Err(Error::Plan)
    ));
}

#[test]
fn wildcard_endpoint_and_fixture_side_effect_fail_closed() {
    let mut bad = plan();
    bad.endpoint_policy.hostname = "*.example.internal".into();
    bad.endpoint_policy.server_name = bad.endpoint_policy.hostname.clone();
    bad.endpoint_policy = bad.endpoint_policy.sealed();
    bad = bad.sealed(&policy());
    let mut owner = TransportAdapterCertification::new(policy()).unwrap();
    assert_eq!(
        owner.apply(&TransportCommand::Register {
            command_id: TransportCommandId(id(1)),
            plan: Box::new(bad),
            recorded_at_ns: 1_100
        }),
        Err(Error::Plan)
    );
    let mut owner = TransportAdapterCertification::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let mut bad = fixture(0);
    bad.socket_opened = true;
    bad = bad.sealed();
    assert_eq!(
        owner.apply(&TransportCommand::RecordFixture {
            command_id: TransportCommandId(id(2)),
            recorded_at_ns: bad.observed_at_ns,
            fixture: Box::new(bad)
        }),
        Err(Error::Fixture)
    );
}

#[test]
fn order_semantics_and_unknown_reconciliation_are_mandatory() {
    let mut owner = TransportAdapterCertification::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let wrong = fixture(1);
    assert_eq!(
        owner.apply(&TransportCommand::RecordFixture {
            command_id: TransportCommandId(id(2)),
            recorded_at_ns: wrong.observed_at_ns,
            fixture: Box::new(wrong)
        }),
        Err(Error::Fixture)
    );
    let mut owner = TransportAdapterCertification::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    for index in 0..11 {
        let evidence = fixture(index);
        owner
            .apply(&TransportCommand::RecordFixture {
                command_id: TransportCommandId(id(50 + u8::try_from(index).unwrap())),
                recorded_at_ns: evidence.observed_at_ns,
                fixture: Box::new(evidence),
            })
            .unwrap();
    }
    assert_eq!(
        owner.apply(&TransportCommand::Finalize {
            command_id: TransportCommandId(id(80)),
            certificate_id: id(81),
            certified_at_ns: 1_130,
            recorded_at_ns: 1_130
        }),
        Err(Error::Finalize)
    );

    let mut owner = TransportAdapterCertification::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    for index in 0..11 {
        let evidence = fixture(index);
        owner
            .apply(&TransportCommand::RecordFixture {
                command_id: TransportCommandId(id(50 + u8::try_from(index).unwrap())),
                recorded_at_ns: evidence.observed_at_ns,
                fixture: Box::new(evidence),
            })
            .unwrap();
    }
    let mut wrong_reconciliation = fixture(11);
    wrong_reconciliation.ambiguity_digest = Some(id(95));
    wrong_reconciliation = wrong_reconciliation.sealed();
    assert_eq!(
        owner.apply(&TransportCommand::RecordFixture {
            command_id: TransportCommandId(id(80)),
            recorded_at_ns: wrong_reconciliation.observed_at_ns,
            fixture: Box::new(wrong_reconciliation),
        }),
        Err(Error::Fixture)
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
fn durable_replay_checkpoint_and_sync_failure_are_fail_closed() {
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
    let recovery = TransportRecovery {
        owner: TransportAdapterCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableTransportCertification::new(writer, recovery).unwrap();
    durable.apply(&register()).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let checkpoint = TransportCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).unwrap();
    assert_eq!(recovered.owner.snapshot().digest, expected);
    let recovery = TransportRecovery {
        owner: TransportAdapterCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing =
        DurableTransportCertification::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&register()),
        Err(TransportStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&register()),
        Err(TransportStorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn backoff_above_policy_never_contributes(backoff in 1_i64..1_000) {
        let mut owner = TransportAdapterCertification::new(policy()).unwrap(); owner.apply(&register()).unwrap();
        for index in 0..8 { let evidence = fixture(index); owner.apply(&TransportCommand::RecordFixture { command_id: TransportCommandId(id(70 + u8::try_from(index).unwrap())), recorded_at_ns: evidence.observed_at_ns, fixture: Box::new(evidence) }).unwrap(); }
        let mut timeout = fixture(8); timeout.backoff_ns = Some(backoff); timeout = timeout.sealed();
        let result = owner.apply(&TransportCommand::RecordFixture { command_id: TransportCommandId(id(88)), recorded_at_ns: timeout.observed_at_ns, fixture: Box::new(timeout) });
        prop_assert_eq!(result.is_ok(), backoff <= policy().maximum_backoff_ns);
    }
}
