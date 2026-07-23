use super::*;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> SessionPolicy {
    SessionPolicy {
        maximum_gateway_report_age_ns: 3_000,
        maximum_plan_lifetime_ns: 2_000,
        maximum_attestation_lifetime_ns: 1_800,
        maximum_lease_lifetime_ns: 200,
        maximum_heartbeat_age_ns: 50,
        maximum_rotations: 4,
    }
}

fn gateway_report() -> GatewayCertificationReport {
    GatewayCertificationReport {
        report_id: id(1),
        plan_digest: id(2),
        broker_report_digest: id(3),
        transport_certificate_digest: id(4),
        authentication_contract_digest: id(5),
        channel_binding_digest: id(6),
        token_binding_digest: id(7),
        fixture_chain_digest: id(8),
        submission_chain_digest: id(9),
        completed_envelopes: 2,
        rejected_envelopes: 0,
        reconciled_unknowns: 1,
        finalized_at_ns: 1_000,
        status: GatewayReportStatus::ShadowCertified,
        credential_material_created: false,
        signature_produced: false,
        socket_opened: false,
        authentication_authority_granted: false,
        external_submission_authority_granted: false,
        deployment_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed()
}

fn attestation(
    epoch: u64,
    predecessor: [u8; 32],
    identity: u8,
    observed: i64,
) -> RecordedSessionAttestation {
    let report = gateway_report();
    RecordedSessionAttestation {
        epoch,
        attestation_id: id(identity),
        predecessor_digest: predecessor,
        gateway_report_digest: report.report_digest,
        authentication_contract_digest: report.authentication_contract_digest,
        channel_binding_digest: report.channel_binding_digest,
        token_binding_digest: report.token_binding_digest,
        observed_at_ns: observed,
        valid_until_ns: 2_900,
        source_digest: id(identity + 1),
        recorded_only: true,
        credential_material_present: false,
        certificate_private_key_present: false,
        signature_bytes_present: false,
        provider_contacted: false,
        socket_opened: false,
        external_authority_granted: false,
        attestation_digest: [0; 32],
    }
    .sealed()
}

fn plan() -> ShadowSessionPlan {
    ShadowSessionPlan {
        plan_id: id(20),
        created_at_ns: 1_100,
        expires_at_ns: 3_000,
        gateway_report: gateway_report(),
        initial_attestation: attestation(0, [0; 32], 21, 1_100),
        required_scenarios: SessionScenario::ALL.to_vec(),
        session_policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn register() -> SessionCommand {
    SessionCommand::Register {
        command_id: SessionCommandId(id(30)),
        plan: Box::new(plan()),
        recorded_at_ns: 1_100,
    }
}

fn open(owner: &mut ShadowAuthSessionCoordinator, identity: u8, at: i64) -> ShadowSessionLease {
    match owner
        .apply(&SessionCommand::OpenLease {
            command_id: SessionCommandId(id(identity)),
            lease_id: id(identity + 1),
            opaque_owner_id: id(identity + 2),
            opened_at_ns: at,
            requested_expires_at_ns: at + 150,
            recorded_at_ns: at,
        })
        .unwrap()
        .detail
    {
        SessionDetail::LeaseOpened(value) => *value,
        _ => panic!("lease"),
    }
}

fn heartbeat(
    lease: &ShadowSessionLease,
    identity: u8,
    at: i64,
    health: HeartbeatHealth,
) -> RecordedSessionHeartbeat {
    RecordedSessionHeartbeat {
        heartbeat_id: id(identity),
        lease_digest: lease.lease_digest,
        sequence: lease.heartbeat_sequence + 1,
        observed_at_ns: at,
        health,
        source_digest: id(identity + 1),
        recorded_only: true,
        credential_loaded: false,
        signature_produced: false,
        provider_contacted: false,
        socket_opened: false,
        authenticated_transport_used: false,
        external_submission_observed: false,
        external_mutation_observed: false,
        heartbeat_digest: [0; 32],
    }
    .sealed()
}

fn requirement(outcome: SessionOutcome) -> SessionRecoveryRequirement {
    match outcome.detail {
        SessionDetail::LeaseRevoked(value) => *value,
        _ => panic!("requirement"),
    }
}

fn recover(
    owner: &mut ShadowAuthSessionCoordinator,
    requirement: SessionRecoveryRequirement,
    identity: u8,
    at: i64,
) {
    let attestation = owner.snapshot().current_attestation.unwrap();
    let evidence = RecordedSessionRecovery {
        recovery_id: id(identity),
        requirement_digest: requirement.requirement_digest,
        subject_digest: requirement.subject_digest,
        attestation_digest: attestation.attestation_digest,
        no_mutation_digest: id(identity + 1),
        opaque_operator_id: id(identity + 2),
        observed_at_ns: at,
        recorded_only: true,
        credential_loaded: false,
        signature_produced: false,
        provider_contacted: false,
        socket_opened: false,
        authenticated_transport_used: false,
        external_submission_observed: false,
        external_mutation_observed: false,
        recovery_digest: [0; 32],
    }
    .sealed();
    owner
        .apply(&SessionCommand::Recover {
            command_id: SessionCommandId(id(identity + 3)),
            requirement: Box::new(requirement),
            evidence,
            recorded_at_ns: at,
        })
        .unwrap();
    assert!(owner.snapshot().active_lease.is_none());
    assert!(owner.snapshot().recovery_requirement.is_none());
}

fn run_complete() -> ShadowAuthSessionCoordinator {
    let mut owner = ShadowAuthSessionCoordinator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();

    let lease = open(&mut owner, 40, 1_200);
    let beat = heartbeat(&lease, 43, 1_210, HeartbeatHealth::Healthy);
    let lease = match owner
        .apply(&SessionCommand::Heartbeat {
            command_id: SessionCommandId(id(45)),
            lease: Box::new(lease),
            heartbeat: beat,
            recorded_at_ns: 1_210,
        })
        .unwrap()
        .detail
    {
        SessionDetail::HeartbeatAccepted(value) => *value,
        _ => panic!("heartbeat"),
    };
    owner
        .apply(&SessionCommand::CloseLease {
            command_id: SessionCommandId(id(46)),
            lease: Box::new(lease),
            closed_at_ns: 1_220,
            recorded_at_ns: 1_220,
        })
        .unwrap();

    let prior = owner.snapshot().current_attestation.unwrap();
    owner
        .apply(&SessionCommand::RotateAttestation {
            command_id: SessionCommandId(id(47)),
            attestation: Box::new(attestation(1, prior.attestation_digest, 48, 1_230)),
            recorded_at_ns: 1_230,
        })
        .unwrap();

    let lease = open(&mut owner, 50, 1_240);
    let ambiguity = requirement(
        owner
            .apply(&SessionCommand::ObserveAmbiguity {
                command_id: SessionCommandId(id(53)),
                lease: Box::new(lease),
                ambiguity_id: id(54),
                ambiguity_digest: id(55),
                observed_at_ns: 1_250,
                recorded_at_ns: 1_250,
            })
            .unwrap(),
    );
    recover(&mut owner, ambiguity, 56, 1_251);

    let _lease = open(&mut owner, 60, 1_260);
    let restart = requirement(
        owner
            .apply(&SessionCommand::Restart {
                command_id: SessionCommandId(id(63)),
                restart_id: id(64),
                restarted_at_ns: 1_270,
                recorded_at_ns: 1_270,
            })
            .unwrap(),
    );
    recover(&mut owner, restart, 65, 1_271);

    let _lease = open(&mut owner, 70, 1_280);
    let deadman = requirement(
        owner
            .apply(&SessionCommand::EvaluateDeadMan {
                command_id: SessionCommandId(id(73)),
                evaluated_at_ns: 1_331,
                recorded_at_ns: 1_331,
            })
            .unwrap(),
    );
    recover(&mut owner, deadman, 74, 1_332);
    owner
}

#[test]
fn complete_session_campaign_covers_safety_without_external_authority() {
    let mut owner = run_complete();
    assert_eq!(
        owner.snapshot().covered_scenarios,
        SessionScenario::ALL.into_iter().collect()
    );
    let report = match owner
        .apply(&SessionCommand::Finalize {
            command_id: SessionCommandId(id(80)),
            report_id: id(81),
            finalized_at_ns: 1_340,
            recorded_at_ns: 1_340,
        })
        .unwrap()
        .detail
    {
        SessionDetail::Finalized(value) => *value,
        _ => panic!("report"),
    };
    assert!(report.verify_digest());
    assert_eq!(report.status, SessionReportStatus::SimulationCompleted);
    assert_eq!(report.rotation_count, 1);
    assert_eq!(report.recovery_count, 3);
    assert!(!report.credential_material_created);
    assert!(!report.signature_produced);
    assert!(!report.provider_contacted);
    assert!(!report.socket_opened);
    assert!(!report.authentication_authority_granted);
    assert!(!report.external_submission_authority_granted);
    assert!(!report.deployment_authority_granted);
    assert!(!report.trading_authority_granted);

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
        Err(SessionReportFileError::Checksum)
    ));
}

#[test]
fn authority_bearing_upstream_and_attestation_substitution_halt() {
    let mut bad = plan();
    bad.gateway_report.authentication_authority_granted = true;
    bad.gateway_report = bad.gateway_report.sealed();
    bad = bad.sealed(&policy());
    let mut owner = ShadowAuthSessionCoordinator::new(policy()).unwrap();
    assert_eq!(
        owner.apply(&SessionCommand::Register {
            command_id: SessionCommandId(id(1)),
            plan: Box::new(bad),
            recorded_at_ns: 1_100,
        }),
        Err(Error::Upstream)
    );

    let mut bad = plan();
    bad.initial_attestation.channel_binding_digest = id(99);
    bad.initial_attestation = bad.initial_attestation.sealed();
    bad = bad.sealed(&policy());
    let mut owner = ShadowAuthSessionCoordinator::new(policy()).unwrap();
    assert_eq!(
        owner.apply(&SessionCommand::Register {
            command_id: SessionCommandId(id(2)),
            plan: Box::new(bad),
            recorded_at_ns: 1_100,
        }),
        Err(Error::Plan)
    );
}

#[test]
fn exclusivity_rotation_and_recovery_order_fail_closed() {
    let mut owner = ShadowAuthSessionCoordinator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let _lease = open(&mut owner, 40, 1_200);
    assert_eq!(
        owner.apply(&SessionCommand::OpenLease {
            command_id: SessionCommandId(id(90)),
            lease_id: id(91),
            opaque_owner_id: id(92),
            opened_at_ns: 1_201,
            requested_expires_at_ns: 1_300,
            recorded_at_ns: 1_201,
        }),
        Err(Error::Lease)
    );

    let mut owner = ShadowAuthSessionCoordinator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let prior = owner.snapshot().current_attestation.unwrap();
    let mut wrong = attestation(1, prior.attestation_digest, 93, 1_200);
    wrong.token_binding_digest = id(94);
    wrong = wrong.sealed();
    assert_eq!(
        owner.apply(&SessionCommand::RotateAttestation {
            command_id: SessionCommandId(id(95)),
            attestation: Box::new(wrong),
            recorded_at_ns: 1_200,
        }),
        Err(Error::Rotation)
    );

    let mut owner = ShadowAuthSessionCoordinator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let _lease = open(&mut owner, 96, 1_200);
    let requirement = requirement(
        owner
            .apply(&SessionCommand::Restart {
                command_id: SessionCommandId(id(99)),
                restart_id: id(100),
                restarted_at_ns: 1_210,
                recorded_at_ns: 1_210,
            })
            .unwrap(),
    );
    assert_eq!(
        owner.apply(&SessionCommand::OpenLease {
            command_id: SessionCommandId(id(101)),
            lease_id: id(102),
            opaque_owner_id: id(103),
            opened_at_ns: 1_211,
            requested_expires_at_ns: 1_300,
            recorded_at_ns: 1_211,
        }),
        Err(Error::Lease)
    );
    assert!(requirement.verify_digest());
}

#[test]
fn unhealthy_heartbeat_revokes_and_side_effect_claims_halt() {
    let mut owner = ShadowAuthSessionCoordinator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let lease = open(&mut owner, 110, 1_200);
    let beat = heartbeat(&lease, 113, 1_210, HeartbeatHealth::Unhealthy);
    let requirement = requirement(
        owner
            .apply(&SessionCommand::Heartbeat {
                command_id: SessionCommandId(id(115)),
                lease: Box::new(lease),
                heartbeat: beat,
                recorded_at_ns: 1_210,
            })
            .unwrap(),
    );
    assert_eq!(requirement.reason, RecoveryReason::UnhealthyHeartbeat);
    assert!(owner.snapshot().active_lease.is_none());

    let mut owner = ShadowAuthSessionCoordinator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let lease = open(&mut owner, 116, 1_200);
    let mut beat = heartbeat(&lease, 119, 1_210, HeartbeatHealth::Healthy);
    beat.socket_opened = true;
    beat = beat.sealed();
    assert_eq!(
        owner.apply(&SessionCommand::Heartbeat {
            command_id: SessionCommandId(id(121)),
            lease: Box::new(lease),
            heartbeat: beat,
            recorded_at_ns: 1_210,
        }),
        Err(Error::Heartbeat)
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
    let recovery = SessionRecovery {
        owner: ShadowAuthSessionCoordinator::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableShadowAuthSession::new(writer, recovery).unwrap();
    durable.apply(&register()).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let checkpoint = SessionCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let path = dir.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&path).unwrap(), checkpoint);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).unwrap();
    assert_eq!(recovered.owner.snapshot().digest, expected);

    let recovery = SessionRecovery {
        owner: ShadowAuthSessionCoordinator::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableShadowAuthSession::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&register()),
        Err(SessionStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&register()),
        Err(SessionStorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn overlong_lease_never_opens(extra in 1_i64..1_000) {
        let mut owner = ShadowAuthSessionCoordinator::new(policy()).unwrap();
        owner.apply(&register()).unwrap();
        let result = owner.apply(&SessionCommand::OpenLease {
            command_id: SessionCommandId(id(130)),
            lease_id: id(131),
            opaque_owner_id: id(132),
            opened_at_ns: 1_200,
            requested_expires_at_ns: 1_400 + extra,
            recorded_at_ns: 1_200,
        });
        prop_assert!(result.is_err());
    }
}
