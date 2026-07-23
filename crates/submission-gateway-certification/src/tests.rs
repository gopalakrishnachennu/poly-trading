use super::*;
use credential_broker_simulator::{
    OpaqueKeyHandle, SigningPolicyContract, SigningPurpose, SigningRequest, SimulatedAlgorithm,
};
use executor_session_simulator::{ExecutorSessionDossier, SessionDossierStatus};
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;
use transport_adapter_certification::{
    CanonicalRequestBinding, EndpointPolicy, HttpMethod, TlsMinimumVersion,
    TransportAdapterCertificate,
};

fn id(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> GatewayPolicy {
    GatewayPolicy {
        maximum_transport_age_ns: 2_000,
        maximum_broker_age_ns: 2_000,
        maximum_campaign_age_ns: 1_000,
        maximum_envelope_lifetime_ns: 500,
        maximum_backoff_ns: 200,
        maximum_envelopes: 8,
    }
}

fn transport_policy() -> TransportCertificationPolicy {
    TransportCertificationPolicy {
        maximum_dossier_age_ns: 2_000,
        maximum_campaign_age_ns: 2_000,
        maximum_backoff_ns: 500,
        maximum_endpoints: 8,
        maximum_pins: 8,
        maximum_bindings: 8,
    }
}

fn broker_policy() -> BrokerPolicy {
    BrokerPolicy {
        maximum_certificate_age_ns: 2_000,
        maximum_campaign_age_ns: 2_000,
        maximum_approval_age_ns: 500,
        maximum_permit_lifetime_ns: 100,
        maximum_requests: 8,
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
        finalized_at_ns: 800,
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

fn transport_plan() -> TransportCertificationPlan {
    TransportCertificationPlan {
        plan_id: id(23),
        created_at_ns: 900,
        expires_at_ns: 2_000,
        session_dossier: dossier(),
        endpoint_policy: endpoint(),
        request_bindings: vec![
            CanonicalRequestBinding {
                sequence: 0,
                template_digest: id(10),
                method: HttpMethod::Post,
                path: "/v1/apply".into(),
                body_digest: id(24),
                canonical_bytes_digest: id(25),
                binding_digest: [0; 32],
            }
            .sealed(),
            CanonicalRequestBinding {
                sequence: 1,
                template_digest: id(11),
                method: HttpMethod::Get,
                path: "/v1/verify".into(),
                body_digest: id(26),
                canonical_bytes_digest: id(27),
                binding_digest: [0; 32],
            }
            .sealed(),
        ],
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&transport_policy())
}

fn certificate() -> TransportAdapterCertificate {
    let transport = transport_plan();
    TransportAdapterCertificate {
        certificate_id: id(28),
        plan_digest: transport.plan_digest,
        session_dossier_digest: transport.session_dossier.dossier_digest,
        endpoint_policy_digest: transport.endpoint_policy.policy_digest,
        fixture_chain_digest: id(29),
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

fn signing_request(sequence: u32, value: u8, purpose: SigningPurpose) -> SigningRequest {
    SigningRequest {
        sequence,
        request_id: id(value),
        purpose,
        subject_digest: id(40 + u8::try_from(sequence).unwrap()),
        payload_digest: id(value + 1),
        nonce_digest: id(value + 2),
        units: 50,
        not_before_ns: 1_100,
        expires_at_ns: 1_900,
        request_digest: [0; 32],
    }
    .sealed()
}

fn broker_plan() -> BrokerPlan {
    BrokerPlan {
        plan_id: id(30),
        created_at_ns: 1_100,
        expires_at_ns: 1_900,
        transport_certificate: certificate(),
        key_handle: OpaqueKeyHandle {
            handle_digest: id(31),
            provider_attestation_digest: id(32),
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
                SigningPurpose::DeploymentRequest,
                SigningPurpose::HealthVerification,
            ],
            allowed_subject_digests: vec![id(40), id(41)],
            maximum_units_per_request: 100,
            maximum_total_units: 100,
            valid_from_ns: 1_000,
            valid_until_ns: 2_000,
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
            signing_request(0, 50, SigningPurpose::DeploymentRequest),
            signing_request(1, 60, SigningPurpose::HealthVerification),
        ],
        broker_policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&broker_policy())
}

fn receipts() -> Vec<SimulatedSigningReceipt> {
    broker_plan()
        .requests
        .iter()
        .enumerate()
        .map(|(index, request)| {
            let sequence = u8::try_from(index).unwrap();
            SimulatedSigningReceipt {
                receipt_id: id(70 + sequence),
                permit_digest: id(75 + sequence),
                request_digest: request.request_digest,
                nonce_digest: request.nonce_digest,
                consumed_at_ns: 1_201 + i64::try_from(index).unwrap() * 20,
                simulated_only: true,
                signature_bytes_present: false,
                key_material_accessed: false,
                provider_contacted: false,
                authentication_authority_granted: false,
                external_submission_authority_granted: false,
                receipt_digest: [0; 32],
            }
            .sealed()
        })
        .collect()
}

fn broker_report() -> BrokerCertificationReport {
    let broker = broker_plan();
    let receipts = receipts();
    BrokerCertificationReport {
        report_id: id(80),
        plan_digest: broker.plan_digest,
        transport_certificate_digest: broker.transport_certificate.certificate_digest,
        key_descriptor_digest: broker.key_handle.descriptor_digest,
        fixture_chain_digest: id(81),
        receipt_chain_digest: simulated_receipt_chain_digest(&receipts),
        completed_request_count: receipts.len(),
        finalized_at_ns: 1_300,
        status: BrokerReportStatus::SimulationCompleted,
        key_material_created: false,
        real_signature_produced: false,
        provider_contacted: false,
        authentication_authority_granted: false,
        external_submission_authority_granted: false,
        deployment_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed()
}

fn auth_contract() -> ShadowAuthenticationContract {
    ShadowAuthenticationContract {
        scheme: ShadowAuthenticationScheme::RecordedApiMac,
        channel_binding_digest: id(82),
        token_binding_digest: id(83),
        canonical_header_names: vec!["x-shadow-key".into(), "x-shadow-timestamp".into()],
        credential_material_present: false,
        authorization_header_values_present: false,
        cookie_values_present: false,
        signature_bytes_present: false,
        provider_access_enabled: false,
        socket_access_enabled: false,
        external_submission_enabled: false,
        contract_digest: [0; 32],
    }
    .sealed()
}

fn plan() -> GatewayCertificationPlan {
    let transport = transport_plan();
    let broker = broker_plan();
    let receipts = receipts();
    let contract = auth_contract();
    let envelopes = receipts
        .into_iter()
        .enumerate()
        .map(|(index, receipt)| {
            let sequence = u8::try_from(index).unwrap();
            ShadowAuthenticatedEnvelope {
                sequence: u32::try_from(index).unwrap(),
                envelope_id: id(90 + sequence),
                broker_request_digest: broker.requests[index].request_digest,
                signing_receipt: receipt,
                transport_binding_digest: transport.request_bindings[index].binding_digest,
                endpoint_policy_digest: transport.endpoint_policy.policy_digest,
                channel_binding_digest: contract.channel_binding_digest,
                token_binding_digest: contract.token_binding_digest,
                idempotency_key_digest: id(100 + sequence),
                created_at_ns: 1_400,
                expires_at_ns: 1_800,
                simulated_only: true,
                credential_material_present: false,
                authorization_header_values_present: false,
                signature_bytes_present: false,
                external_submission_authority_granted: false,
                envelope_digest: [0; 32],
            }
            .sealed()
        })
        .collect();
    GatewayCertificationPlan {
        plan_id: id(110),
        created_at_ns: 1_400,
        expires_at_ns: 1_900,
        transport_policy: transport_policy(),
        transport_plan: transport,
        broker_policy: broker_policy(),
        broker_plan: broker,
        broker_report: broker_report(),
        authentication_contract: contract,
        envelopes,
        gateway_policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn register() -> GatewayCommand {
    GatewayCommand::Register {
        command_id: GatewayCommandId(id(120)),
        plan: Box::new(plan()),
        recorded_at_ns: 1_400,
    }
}

fn fixture(index: usize) -> RecordedGatewayFixture {
    let current_plan = plan();
    let envelope = &current_plan.envelopes[0];
    let case = GatewayFixtureCase::ALL[index];
    let sequence = u8::try_from(index).unwrap();
    let mut value = RecordedGatewayFixture {
        sequence,
        case,
        expected: case.expected(),
        observed: case.expected(),
        observed_at_ns: 1_410 + i64::try_from(index).unwrap(),
        envelope_digest: envelope.envelope_digest,
        endpoint_policy_digest: envelope.endpoint_policy_digest,
        channel_binding_digest: envelope.channel_binding_digest,
        token_binding_digest: envelope.token_binding_digest,
        idempotency_key_digest: envelope.idempotency_key_digest,
        envelope_expires_at_ns: envelope.expires_at_ns,
        backoff_ns: None,
        ambiguity_digest: None,
        reconciliation_digest: None,
        fixture_source_digest: id(130 + sequence),
        recorded_fixture: true,
        credential_loaded: false,
        signature_produced: false,
        socket_opened: false,
        authenticated_request_sent: false,
        external_submission_observed: false,
        external_mutation_observed: false,
        fixture_digest: [0; 32],
    };
    match case {
        GatewayFixtureCase::WrongEndpoint => value.endpoint_policy_digest = id(200),
        GatewayFixtureCase::WrongChannelBinding => value.channel_binding_digest = id(201),
        GatewayFixtureCase::WrongTokenBinding => value.token_binding_digest = id(202),
        GatewayFixtureCase::IdempotencyConflict => value.envelope_digest = id(203),
        GatewayFixtureCase::ExpiredEnvelope => {
            value.envelope_expires_at_ns = 1_405;
        }
        GatewayFixtureCase::RateLimited => value.backoff_ns = Some(100),
        GatewayFixtureCase::UnknownResponse => value.ambiguity_digest = Some(id(204)),
        GatewayFixtureCase::NoMutationReconciliation => {
            value.ambiguity_digest = Some(id(204));
            value.reconciliation_digest = Some(id(205));
        }
        GatewayFixtureCase::ValidEnvelope | GatewayFixtureCase::ReceiptReplay => {}
    }
    value.sealed()
}

fn run_fixtures(owner: &mut SubmissionGatewayCertification) {
    owner.apply(&register()).unwrap();
    for index in 0..GatewayFixtureCase::ALL.len() {
        let evidence = fixture(index);
        owner
            .apply(&GatewayCommand::RecordFixture {
                command_id: GatewayCommandId(id(140 + u8::try_from(index).unwrap())),
                recorded_at_ns: evidence.observed_at_ns,
                fixture: Box::new(evidence),
            })
            .unwrap();
    }
}

fn stage(owner: &mut SubmissionGatewayCertification, sequence: usize, at: i64) -> ShadowSubmission {
    match owner
        .apply(&GatewayCommand::StageNext {
            command_id: GatewayCommandId(id(160 + u8::try_from(sequence).unwrap())),
            submission_id: id(170 + u8::try_from(sequence).unwrap()),
            staged_at_ns: at,
            recorded_at_ns: at,
        })
        .unwrap()
        .detail
    {
        GatewayDetail::Staged(value) => *value,
        _ => panic!("submission"),
    }
}

fn observation(
    submission: &ShadowSubmission,
    identity: u8,
    outcome: RecordedSubmissionOutcome,
    at: i64,
) -> RecordedSubmissionObservation {
    RecordedSubmissionObservation {
        observation_id: id(identity),
        submission_digest: submission.submission_digest,
        outcome,
        observed_at_ns: at,
        source_digest: id(identity + 1),
        recorded_fixture: true,
        credential_loaded: false,
        signature_produced: false,
        socket_opened: false,
        authenticated_request_sent: false,
        external_submission_observed: false,
        external_mutation_observed: false,
        observation_digest: [0; 32],
    }
    .sealed()
}

fn unknown_submission(
    owner: &mut SubmissionGatewayCertification,
    submission: ShadowSubmission,
    at: i64,
) -> ShadowSubmission {
    let recorded = observation(&submission, 190, RecordedSubmissionOutcome::Unknown, at);
    owner
        .apply(&GatewayCommand::Observe {
            command_id: GatewayCommandId(id(191)),
            submission: Box::new(submission),
            observation: recorded,
            recorded_at_ns: at,
        })
        .unwrap();
    owner.snapshot().active_submission.unwrap()
}

#[test]
fn complete_gateway_campaign_certifies_without_authentication_or_submission_authority() {
    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    run_fixtures(&mut owner);
    let first = stage(&mut owner, 0, 1_430);
    let accepted = observation(&first, 180, RecordedSubmissionOutcome::Accepted, 1_431);
    owner
        .apply(&GatewayCommand::Observe {
            command_id: GatewayCommandId(id(181)),
            submission: Box::new(first),
            observation: accepted,
            recorded_at_ns: 1_431,
        })
        .unwrap();

    let second = stage(&mut owner, 1, 1_440);
    let unknown = unknown_submission(&mut owner, second, 1_441);
    let evidence = RecordedNoMutationEvidence {
        evidence_id: id(192),
        submission_digest: unknown.submission_digest,
        unknown_observation_digest: unknown.unknown_observation_digest.unwrap(),
        external_state_digest: id(193),
        observed_at_ns: 1_442,
        recorded_fixture: true,
        credential_loaded: false,
        socket_opened: false,
        authenticated_request_sent: false,
        external_submission_observed: false,
        external_mutation_observed: false,
        evidence_digest: [0; 32],
    }
    .sealed();
    owner
        .apply(&GatewayCommand::ReconcileUnknown {
            command_id: GatewayCommandId(id(194)),
            submission: Box::new(unknown),
            evidence,
            recorded_at_ns: 1_442,
        })
        .unwrap();

    let report = match owner
        .apply(&GatewayCommand::Finalize {
            command_id: GatewayCommandId(id(195)),
            report_id: id(196),
            finalized_at_ns: 1_450,
            recorded_at_ns: 1_450,
        })
        .unwrap()
        .detail
    {
        GatewayDetail::Finalized(value) => *value,
        _ => panic!("report"),
    };
    assert!(report.verify_digest());
    assert_eq!(report.status, GatewayReportStatus::ShadowCertified);
    assert_eq!(report.completed_envelopes, 2);
    assert_eq!(report.reconciled_unknowns, 1);
    assert!(!report.credential_material_created);
    assert!(!report.signature_produced);
    assert!(!report.socket_opened);
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
        Err(GatewayReportFileError::Checksum)
    ));
}

#[test]
fn substituted_or_authority_bearing_upstream_halts_before_registration() {
    let mut bad = plan();
    bad.broker_report.authentication_authority_granted = true;
    bad.broker_report = bad.broker_report.sealed();
    bad = bad.sealed(&policy());
    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    assert_eq!(
        owner.apply(&GatewayCommand::Register {
            command_id: GatewayCommandId(id(1)),
            plan: Box::new(bad),
            recorded_at_ns: 1_400,
        }),
        Err(Error::Upstream)
    );

    let mut bad = plan();
    bad.envelopes[0].transport_binding_digest = id(250);
    bad.envelopes[0] = bad.envelopes[0].clone().sealed();
    bad = bad.sealed(&policy());
    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    assert_eq!(
        owner.apply(&GatewayCommand::Register {
            command_id: GatewayCommandId(id(2)),
            plan: Box::new(bad),
            recorded_at_ns: 1_400,
        }),
        Err(Error::Plan)
    );
}

#[test]
fn fixture_order_side_effects_and_incomplete_matrix_fail_closed() {
    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let wrong = fixture(1);
    assert_eq!(
        owner.apply(&GatewayCommand::RecordFixture {
            command_id: GatewayCommandId(id(3)),
            fixture: Box::new(wrong.clone()),
            recorded_at_ns: wrong.observed_at_ns,
        }),
        Err(Error::Fixture)
    );

    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let mut bad = fixture(0);
    bad.socket_opened = true;
    bad = bad.sealed();
    assert_eq!(
        owner.apply(&GatewayCommand::RecordFixture {
            command_id: GatewayCommandId(id(4)),
            fixture: Box::new(bad.clone()),
            recorded_at_ns: bad.observed_at_ns,
        }),
        Err(Error::Fixture)
    );

    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    assert_eq!(
        owner.apply(&GatewayCommand::StageNext {
            command_id: GatewayCommandId(id(5)),
            submission_id: id(6),
            staged_at_ns: 1_420,
            recorded_at_ns: 1_420,
        }),
        Err(Error::Submission)
    );
}

#[test]
fn unknown_blocks_progress_and_substitution_cannot_reconcile() {
    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    run_fixtures(&mut owner);
    let submission = stage(&mut owner, 0, 1_430);
    let _unknown = unknown_submission(&mut owner, submission, 1_431);
    assert_eq!(
        owner.apply(&GatewayCommand::StageNext {
            command_id: GatewayCommandId(id(7)),
            submission_id: id(8),
            staged_at_ns: 1_432,
            recorded_at_ns: 1_432,
        }),
        Err(Error::Submission)
    );

    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    run_fixtures(&mut owner);
    let submission = stage(&mut owner, 0, 1_430);
    let unknown = unknown_submission(&mut owner, submission, 1_431);
    let mut evidence = RecordedNoMutationEvidence {
        evidence_id: id(9),
        submission_digest: unknown.submission_digest,
        unknown_observation_digest: id(10),
        external_state_digest: id(11),
        observed_at_ns: 1_432,
        recorded_fixture: true,
        credential_loaded: false,
        socket_opened: false,
        authenticated_request_sent: false,
        external_submission_observed: false,
        external_mutation_observed: false,
        evidence_digest: [0; 32],
    };
    evidence = evidence.sealed();
    assert_eq!(
        owner.apply(&GatewayCommand::ReconcileUnknown {
            command_id: GatewayCommandId(id(12)),
            submission: Box::new(unknown),
            evidence,
            recorded_at_ns: 1_432,
        }),
        Err(Error::Reconciliation)
    );
}

#[test]
fn submission_id_replay_and_expired_envelope_fail_closed() {
    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    run_fixtures(&mut owner);
    let first = stage(&mut owner, 0, 1_430);
    let accepted = observation(&first, 13, RecordedSubmissionOutcome::Accepted, 1_431);
    owner
        .apply(&GatewayCommand::Observe {
            command_id: GatewayCommandId(id(14)),
            submission: Box::new(first),
            observation: accepted,
            recorded_at_ns: 1_431,
        })
        .unwrap();
    assert_eq!(
        owner.apply(&GatewayCommand::StageNext {
            command_id: GatewayCommandId(id(15)),
            submission_id: id(170),
            staged_at_ns: 1_440,
            recorded_at_ns: 1_440,
        }),
        Err(Error::Submission)
    );

    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    run_fixtures(&mut owner);
    assert_eq!(
        owner.apply(&GatewayCommand::StageNext {
            command_id: GatewayCommandId(id(16)),
            submission_id: id(17),
            staged_at_ns: 1_801,
            recorded_at_ns: 1_801,
        }),
        Err(Error::Submission)
    );
}

#[test]
fn recorded_rejection_is_not_certified_and_observation_side_effects_halt() {
    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    run_fixtures(&mut owner);
    let submission = stage(&mut owner, 0, 1_430);
    let mut bad = observation(&submission, 18, RecordedSubmissionOutcome::Accepted, 1_431);
    bad.external_submission_observed = true;
    bad = bad.sealed();
    assert_eq!(
        owner.apply(&GatewayCommand::Observe {
            command_id: GatewayCommandId(id(19)),
            submission: Box::new(submission),
            observation: bad,
            recorded_at_ns: 1_431,
        }),
        Err(Error::Observation)
    );

    let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
    run_fixtures(&mut owner);
    for sequence in 0..2 {
        let at = 1_430 + i64::try_from(sequence).unwrap() * 10;
        let submission = stage(&mut owner, sequence, at);
        let outcome = if sequence == 0 {
            RecordedSubmissionOutcome::Rejected
        } else {
            RecordedSubmissionOutcome::Accepted
        };
        let recorded = observation(
            &submission,
            20 + u8::try_from(sequence).unwrap() * 2,
            outcome,
            at + 1,
        );
        owner
            .apply(&GatewayCommand::Observe {
                command_id: GatewayCommandId(id(21 + u8::try_from(sequence).unwrap() * 2)),
                submission: Box::new(submission),
                observation: recorded,
                recorded_at_ns: at + 1,
            })
            .unwrap();
    }
    let report = match owner
        .apply(&GatewayCommand::Finalize {
            command_id: GatewayCommandId(id(25)),
            report_id: id(26),
            finalized_at_ns: 1_450,
            recorded_at_ns: 1_450,
        })
        .unwrap()
        .detail
    {
        GatewayDetail::Finalized(value) => *value,
        _ => panic!("report"),
    };
    assert_eq!(report.status, GatewayReportStatus::NotCertified);
    assert_eq!(report.rejected_envelopes, 1);
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
    let recovery = GatewayRecovery {
        owner: SubmissionGatewayCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableSubmissionGateway::new(writer, recovery).unwrap();
    durable.apply(&register()).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let checkpoint = GatewayCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let path = dir.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&path).unwrap(), checkpoint);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).unwrap();
    assert_eq!(recovered.owner.snapshot().digest, expected);

    let recovery = GatewayRecovery {
        owner: SubmissionGatewayCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableSubmissionGateway::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&register()),
        Err(GatewayStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&register()),
        Err(GatewayStorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn backoff_above_policy_never_contributes(backoff in 1_i64..1_000) {
        let mut owner = SubmissionGatewayCertification::new(policy()).unwrap();
        owner.apply(&register()).unwrap();
        for index in 0..7 {
            let evidence = fixture(index);
            owner.apply(&GatewayCommand::RecordFixture {
                command_id: GatewayCommandId(id(210 + u8::try_from(index).unwrap())),
                recorded_at_ns: evidence.observed_at_ns,
                fixture: Box::new(evidence),
            }).unwrap();
        }
        let mut rate = fixture(7);
        rate.backoff_ns = Some(backoff);
        rate = rate.sealed();
        let result = owner.apply(&GatewayCommand::RecordFixture {
            command_id: GatewayCommandId(id(220)),
            recorded_at_ns: rate.observed_at_ns,
            fixture: Box::new(rate),
        });
        prop_assert_eq!(result.is_ok(), backoff <= policy().maximum_backoff_ns);
    }
}
