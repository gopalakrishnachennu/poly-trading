use super::*;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> BrokerPolicy {
    BrokerPolicy {
        maximum_certificate_age_ns: 2_000,
        maximum_campaign_age_ns: 2_000,
        maximum_approval_age_ns: 500,
        maximum_permit_lifetime_ns: 100,
        maximum_requests: 8,
    }
}

fn certificate() -> TransportAdapterCertificate {
    TransportAdapterCertificate {
        certificate_id: id(1),
        plan_digest: id(2),
        session_dossier_digest: id(3),
        endpoint_policy_digest: id(4),
        fixture_chain_digest: id(5),
        fixture_count: 12,
        certified_at_ns: 1_000,
        status: TransportCertificateStatus::RecordedFixtureCertified,
        recorded_fixtures_only: true,
        socket_authority_granted: false,
        credential_material_created: false,
        authentication_authority_granted: false,
        external_submission_authority_granted: false,
        deployment_authority_granted: false,
        certificate_digest: [0; 32],
    }
    .sealed()
}

fn request(
    sequence: u32,
    request_id: u8,
    purpose: SigningPurpose,
    subject: u8,
    nonce: u8,
) -> SigningRequest {
    SigningRequest {
        sequence,
        request_id: id(request_id),
        purpose,
        subject_digest: id(subject),
        payload_digest: id(request_id + 1),
        nonce_digest: id(nonce),
        units: 50,
        not_before_ns: 1_100,
        expires_at_ns: 1_800,
        request_digest: [0; 32],
    }
    .sealed()
}

fn plan() -> BrokerPlan {
    BrokerPlan {
        plan_id: id(10),
        created_at_ns: 1_100,
        expires_at_ns: 1_800,
        transport_certificate: certificate(),
        key_handle: OpaqueKeyHandle {
            handle_digest: id(11),
            provider_attestation_digest: id(12),
            algorithm: SimulatedAlgorithm::Ed25519,
            key_material_present: false,
            exportable: false,
            provider_access_enabled: false,
            initially_revoked: false,
            descriptor_digest: [0; 32],
        }
        .sealed(),
        signing_policy: SigningPolicyContract {
            allowed_purposes: vec![
                SigningPurpose::HealthVerification,
                SigningPurpose::DeploymentRequest,
            ],
            allowed_subject_digests: vec![id(21), id(20)],
            maximum_units_per_request: 100,
            maximum_total_units: 150,
            valid_from_ns: 1_000,
            valid_until_ns: 3_000,
            dual_authorization_required: true,
            arbitrary_payload_allowed: false,
            transfer_allowed: false,
            withdrawal_allowed: false,
            wallet_access_allowed: false,
            external_submission_allowed: false,
            policy_digest: [0; 32],
        }
        .sealed(),
        requests: vec![
            request(0, 30, SigningPurpose::DeploymentRequest, 20, 32),
            request(1, 33, SigningPurpose::HealthVerification, 21, 35),
        ],
        broker_policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn register() -> BrokerCommand {
    BrokerCommand::Register {
        command_id: BrokerCommandId(id(40)),
        plan: Box::new(plan()),
        recorded_at_ns: 1_100,
    }
}

fn fixture(index: usize) -> RecordedSignerFixture {
    let case = SignerFixtureCase::ALL[index];
    let sequence = u8::try_from(index).unwrap();
    RecordedSignerFixture {
        sequence,
        case,
        expected: case.expected(),
        observed: case.expected(),
        observed_at_ns: 1_110 + i64::try_from(index).unwrap(),
        fixture_source_digest: id(50 + sequence),
        recorded_fixture: true,
        key_material_accessed: false,
        provider_contacted: false,
        real_signature_produced: false,
        credential_created: false,
        authenticated_transport_used: false,
        external_submission_observed: false,
        fixture_digest: [0; 32],
    }
    .sealed()
}

fn run_fixtures(owner: &mut CredentialBrokerSimulator) {
    owner.apply(&register()).unwrap();
    for index in 0..SignerFixtureCase::ALL.len() {
        let evidence = fixture(index);
        owner
            .apply(&BrokerCommand::RecordFixture {
                command_id: BrokerCommandId(id(60 + u8::try_from(index).unwrap())),
                recorded_at_ns: evidence.observed_at_ns,
                fixture: Box::new(evidence),
            })
            .unwrap();
    }
}

fn authorization(
    request: &SigningRequest,
    role: AuthorizationRole,
    authorization_id: u8,
    operator_id: u8,
    authorized_at_ns: i64,
) -> RequestAuthorization {
    RequestAuthorization {
        authorization_id: id(authorization_id),
        plan_digest: plan().plan_digest,
        request_digest: request.request_digest,
        role,
        operator_id: id(operator_id),
        approved: true,
        authorized_at_ns,
        valid_until_ns: 1_700,
        authorization_digest: [0; 32],
    }
    .sealed()
}

fn authorize_current(owner: &mut CredentialBrokerSimulator, sequence: usize, at: i64) {
    let request = plan().requests[sequence].clone();
    let sequence_id = u8::try_from(sequence).unwrap();
    for (offset, role, operator) in [
        (0_u8, AuthorizationRole::Security, 90_u8),
        (1_u8, AuthorizationRole::Operations, 91_u8),
    ] {
        owner
            .apply(&BrokerCommand::Authorize {
                command_id: BrokerCommandId(id(100 + sequence_id * 4 + offset)),
                authorization: authorization(
                    &request,
                    role,
                    120 + sequence_id * 4 + offset,
                    operator,
                    at,
                ),
                recorded_at_ns: at,
            })
            .unwrap();
    }
}

fn issue(
    owner: &mut CredentialBrokerSimulator,
    sequence: usize,
    at: i64,
) -> SimulatedSigningPermit {
    let sequence_id = u8::try_from(sequence).unwrap();
    match owner
        .apply(&BrokerCommand::IssuePermit {
            command_id: BrokerCommandId(id(140 + sequence_id)),
            permit_id: id(150 + sequence_id),
            issued_at_ns: at,
            requested_expires_at_ns: at + 100,
            recorded_at_ns: at,
        })
        .unwrap()
        .detail
    {
        BrokerDetail::PermitIssued(value) => *value,
        _ => panic!("permit"),
    }
}

fn consume(
    owner: &mut CredentialBrokerSimulator,
    sequence: usize,
    permit: SimulatedSigningPermit,
    at: i64,
) -> SimulatedSigningReceipt {
    let sequence_id = u8::try_from(sequence).unwrap();
    match owner
        .apply(&BrokerCommand::ConsumePermit {
            command_id: BrokerCommandId(id(160 + sequence_id)),
            permit: Box::new(permit),
            receipt_id: id(170 + sequence_id),
            consumed_at_ns: at,
            recorded_at_ns: at,
        })
        .unwrap()
        .detail
    {
        BrokerDetail::PermitConsumed(value) => *value,
        _ => panic!("receipt"),
    }
}

#[test]
fn complete_simulation_has_no_key_signature_transport_or_submission_authority() {
    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    run_fixtures(&mut owner);
    for sequence in 0..2 {
        let at = 1_200 + i64::try_from(sequence).unwrap() * 150;
        authorize_current(&mut owner, sequence, at);
        let permit = issue(&mut owner, sequence, at);
        assert!(permit.verify_digest());
        assert!(permit.one_use && permit.simulated_only);
        let receipt = consume(&mut owner, sequence, permit, at + 1);
        assert!(receipt.verify_digest());
        assert!(receipt.simulated_only);
        assert!(!receipt.signature_bytes_present);
        assert!(!receipt.key_material_accessed);
        assert!(!receipt.provider_contacted);
        assert!(!receipt.authentication_authority_granted);
        assert!(!receipt.external_submission_authority_granted);
    }
    let report = match owner
        .apply(&BrokerCommand::Finalize {
            command_id: BrokerCommandId(id(180)),
            report_id: id(181),
            finalized_at_ns: 1_501,
            recorded_at_ns: 1_501,
        })
        .unwrap()
        .detail
    {
        BrokerDetail::Finalized(value) => *value,
        _ => panic!("report"),
    };
    assert!(report.verify_digest());
    assert_eq!(report.status, BrokerReportStatus::SimulationCompleted);
    assert!(!report.key_material_created);
    assert!(!report.real_signature_produced);
    assert!(!report.provider_contacted);
    assert!(!report.authentication_authority_granted);
    assert!(!report.external_submission_authority_granted);
    assert!(!report.deployment_authority_granted);

    let dir = tempdir().unwrap();
    let path = dir.path().join("report.bin");
    write_report_create_new(&path, &report).unwrap();
    assert_eq!(read_report(&path).unwrap(), report);
    assert!(write_report_create_new(&path, &report).is_err());
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        read_report(&path),
        Err(BrokerReportFileError::Checksum)
    ));
}

#[test]
fn upstream_authority_and_key_material_are_rejected_before_registration() {
    let mut bad = plan();
    bad.transport_certificate
        .external_submission_authority_granted = true;
    bad.transport_certificate = bad.transport_certificate.sealed();
    bad = bad.sealed(&policy());
    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    assert_eq!(
        owner.apply(&BrokerCommand::Register {
            command_id: BrokerCommandId(id(1)),
            plan: Box::new(bad),
            recorded_at_ns: 1_100,
        }),
        Err(Error::Upstream)
    );

    let mut bad = plan();
    bad.key_handle.key_material_present = true;
    bad.key_handle = bad.key_handle.sealed();
    bad = bad.sealed(&policy());
    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    assert_eq!(
        owner.apply(&BrokerCommand::Register {
            command_id: BrokerCommandId(id(2)),
            plan: Box::new(bad),
            recorded_at_ns: 1_100,
        }),
        Err(Error::Plan)
    );
}

#[test]
fn fixture_matrix_order_completeness_and_side_effects_fail_closed() {
    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let wrong = fixture(1);
    assert_eq!(
        owner.apply(&BrokerCommand::RecordFixture {
            command_id: BrokerCommandId(id(2)),
            recorded_at_ns: wrong.observed_at_ns,
            fixture: Box::new(wrong),
        }),
        Err(Error::Fixture)
    );

    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let mut bad = fixture(0);
    bad.provider_contacted = true;
    bad = bad.sealed();
    assert_eq!(
        owner.apply(&BrokerCommand::RecordFixture {
            command_id: BrokerCommandId(id(3)),
            recorded_at_ns: bad.observed_at_ns,
            fixture: Box::new(bad),
        }),
        Err(Error::Fixture)
    );

    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    assert_eq!(
        owner.apply(&BrokerCommand::Finalize {
            command_id: BrokerCommandId(id(4)),
            report_id: id(5),
            finalized_at_ns: 1_120,
            recorded_at_ns: 1_120,
        }),
        Err(Error::Finalize)
    );
}

#[test]
fn dual_control_staleness_permit_expiry_and_replay_fail_closed() {
    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    run_fixtures(&mut owner);
    let request = plan().requests[0].clone();
    let first = authorization(&request, AuthorizationRole::Security, 100, 90, 1_200);
    owner
        .apply(&BrokerCommand::Authorize {
            command_id: BrokerCommandId(id(101)),
            authorization: first,
            recorded_at_ns: 1_200,
        })
        .unwrap();
    let same_operator = authorization(&request, AuthorizationRole::Operations, 102, 90, 1_200);
    assert_eq!(
        owner.apply(&BrokerCommand::Authorize {
            command_id: BrokerCommandId(id(103)),
            authorization: same_operator,
            recorded_at_ns: 1_200,
        }),
        Err(Error::Authorization)
    );

    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    run_fixtures(&mut owner);
    authorize_current(&mut owner, 0, 1_120);
    assert_eq!(
        owner.apply(&BrokerCommand::IssuePermit {
            command_id: BrokerCommandId(id(104)),
            permit_id: id(105),
            issued_at_ns: 1_621,
            requested_expires_at_ns: 1_650,
            recorded_at_ns: 1_621,
        }),
        Err(Error::Authorization)
    );

    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    run_fixtures(&mut owner);
    authorize_current(&mut owner, 0, 1_200);
    let permit = issue(&mut owner, 0, 1_200);
    assert_eq!(
        owner.apply(&BrokerCommand::ConsumePermit {
            command_id: BrokerCommandId(id(106)),
            permit: Box::new(permit),
            receipt_id: id(107),
            consumed_at_ns: 1_301,
            recorded_at_ns: 1_301,
        }),
        Err(Error::Permit)
    );

    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    run_fixtures(&mut owner);
    authorize_current(&mut owner, 0, 1_200);
    let permit = issue(&mut owner, 0, 1_200);
    let _receipt = consume(&mut owner, 0, permit.clone(), 1_201);
    assert_eq!(
        owner.apply(&BrokerCommand::ConsumePermit {
            command_id: BrokerCommandId(id(108)),
            permit: Box::new(permit),
            receipt_id: id(109),
            consumed_at_ns: 1_202,
            recorded_at_ns: 1_202,
        }),
        Err(Error::Permit)
    );
}

#[test]
fn revocation_discards_active_permit_and_allows_only_revoked_report() {
    let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
    run_fixtures(&mut owner);
    authorize_current(&mut owner, 0, 1_200);
    let _permit = issue(&mut owner, 0, 1_200);
    owner
        .apply(&BrokerCommand::Revoke {
            command_id: BrokerCommandId(id(190)),
            revocation_id: id(191),
            revoked_at_ns: 1_201,
            recorded_at_ns: 1_201,
        })
        .unwrap();
    assert!(owner.snapshot().active_permit.is_none());
    let report = match owner
        .apply(&BrokerCommand::Finalize {
            command_id: BrokerCommandId(id(192)),
            report_id: id(193),
            finalized_at_ns: 1_202,
            recorded_at_ns: 1_202,
        })
        .unwrap()
        .detail
    {
        BrokerDetail::Finalized(value) => *value,
        _ => panic!("report"),
    };
    assert_eq!(report.status, BrokerReportStatus::HandleRevoked);
    assert_eq!(report.completed_request_count, 0);
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
fn journal_replay_checkpoint_and_sync_failure_are_fail_closed() {
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
    let recovery = BrokerRecovery {
        owner: CredentialBrokerSimulator::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableCredentialBroker::new(writer, recovery).unwrap();
    durable.apply(&register()).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let checkpoint = BrokerCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let checkpoint_path = dir.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&checkpoint_path).unwrap(), checkpoint);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).unwrap();
    assert_eq!(recovered.owner.snapshot().digest, expected);

    let recovery = BrokerRecovery {
        owner: CredentialBrokerSimulator::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableCredentialBroker::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&register()),
        Err(BrokerStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&register()),
        Err(BrokerStorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn permit_lifetime_above_policy_never_issues(extra in 1_i64..1_000) {
        let mut owner = CredentialBrokerSimulator::new(policy()).unwrap();
        run_fixtures(&mut owner);
        authorize_current(&mut owner, 0, 1_200);
        let result = owner.apply(&BrokerCommand::IssuePermit {
            command_id: BrokerCommandId(id(200)),
            permit_id: id(201),
            issued_at_ns: 1_200,
            requested_expires_at_ns: 1_300 + extra,
            recorded_at_ns: 1_200,
        });
        prop_assert_eq!(result.is_ok(), false);
    }
}
