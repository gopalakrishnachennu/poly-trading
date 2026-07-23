use super::*;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(v: u8) -> [u8; 32] {
    [v; 32]
}
fn policy() -> VenuePolicy {
    VenuePolicy {
        maximum_security_report_age_ns: 10_000,
        maximum_plan_lifetime_ns: 5_000,
        maximum_channel_age_ns: 100,
        maximum_parameter_age_ns: 100,
        maximum_mode_age_ns: 100,
        maximum_backoff_ns: 100,
    }
}
fn upstream() -> SecurityReport {
    SecurityReport {
        report_id: id(1),
        plan_digest: id(2),
        infrastructure_report_digest: id(3),
        covered_scenarios: SecurityScenario::ALL.to_vec(),
        covered_providers: ProviderClass::ALL.to_vec(),
        final_identity_epoch: 3,
        finalized_at_ns: 100,
        status: SecurityReportStatus::LocallyCertified,
        real_provider_certified: false,
        secret_material_created: false,
        signature_produced: false,
        provider_contacted: false,
        socket_opened: false,
        signer_activated: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        submission_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed()
}
fn contract() -> AuthenticatedObservationContract {
    AuthenticatedObservationContract {
        host_digest: id(10),
        channel_subject_digest: id(11),
        allowed_events: UserEventClass::ALL.to_vec(),
        subscription_only: true,
        credential_value_present: false,
        authorization_header_present: false,
        order_endpoint_present: false,
        cancel_endpoint_present: false,
        wallet_endpoint_present: false,
        arbitrary_request_allowed: false,
        contract_digest: [0; 32],
    }
    .sealed()
}
fn plan() -> VenuePlan {
    VenuePlan {
        plan_id: id(12),
        security_report: upstream(),
        authenticated_contract: contract(),
        condition_id_digest: id(13),
        up_token_digest: id(14),
        down_token_digest: id(15),
        required_scenarios: VenueScenario::ALL.to_vec(),
        created_at_ns: 200,
        expires_at_ns: 4_000,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}
fn channel_byte(v: ChannelKind) -> u8 {
    match v {
        ChannelKind::PublicMarket => 20,
        ChannelKind::AuthenticatedUser => 30,
        ChannelKind::RestMetadata => 40,
        ChannelKind::ReferencePrice => 50,
    }
}
fn channel(v: ChannelKind, epoch: u64, sequence: u64, at: i64) -> ChannelObservation {
    let b = channel_byte(v);
    ChannelObservation {
        observation_id: id(b.wrapping_add(u8::try_from(epoch).unwrap_or(0))),
        channel: v,
        epoch,
        sequence,
        snapshot_digest: id(b + 1),
        provenance_digest: id(b + 2),
        event_time_ns: at - 2,
        received_time_ns: at - 1,
        observed_at_ns: at,
        health: ChannelHealth::Ready,
        observation_digest: [0; 32],
    }
    .sealed()
}
fn parameters(version: u64, at: i64) -> MarketParameters {
    MarketParameters {
        condition_id_digest: plan().condition_id_digest,
        up_token_digest: plan().up_token_digest,
        down_token_digest: plan().down_token_digest,
        version,
        tick_size_micros: 1_000,
        minimum_order_quantity_micros: 1_000_000,
        maker_fee_bps: 0,
        taker_fee_bps: 200,
        taker_delay_ns: 500_000_000,
        minimum_order_age_ns: 1_000_000_000,
        observed_at_ns: at,
        parameters_digest: [0; 32],
    }
    .sealed()
}
fn mode(sequence: u64, value: VenueMode, at: i64) -> ModeObservation {
    ModeObservation {
        observation_id: id(60 + u8::try_from(sequence).unwrap_or(0)),
        sequence,
        mode: value,
        source_digest: id(61),
        observed_at_ns: at,
        observation_digest: [0; 32],
    }
    .sealed()
}
fn registered() -> ReadOnlyVenueSupervisor {
    let mut owner = ReadOnlyVenueSupervisor::new(policy()).unwrap();
    owner
        .apply(&VenueCommand::Register {
            command_id: VenueCommandId(id(70)),
            plan: Box::new(plan()),
            recorded_at_ns: 200,
        })
        .unwrap();
    owner
}
fn sync(owner: &mut ReadOnlyVenueSupervisor, epoch: u64, start: i64, command_base: u8) {
    for (offset, kind) in ChannelKind::ALL.into_iter().enumerate() {
        let at = start + i64::try_from(offset).unwrap();
        owner
            .apply(&VenueCommand::ObserveChannel {
                command_id: VenueCommandId(id(command_base + u8::try_from(offset).unwrap())),
                observation: channel(kind, epoch, 1, at),
                recorded_at_ns: at,
            })
            .unwrap();
    }
}
fn requirement(outcome: VenueOutcome) -> VenueRecoveryRequirement {
    match outcome.detail {
        VenueDetail::RecoveryRequired(v) => *v,
        _ => panic!("requirement"),
    }
}
fn recovery(
    requirement: &VenueRecoveryRequirement,
    epoch: u64,
    parameter_version: u64,
    mode_sequence: u64,
    at: i64,
    identity: u8,
) -> VenueRecoveryEvidence {
    VenueRecoveryEvidence {
        recovery_id: id(identity),
        requirement_digest: requirement.requirement_digest,
        channel_snapshots: ChannelKind::ALL
            .into_iter()
            .map(|kind| channel(kind, epoch, 1, at))
            .collect(),
        parameters: parameters(parameter_version, at),
        mode: mode(mode_sequence, VenueMode::Normal, at),
        reconciliation_digest: id(identity + 1),
        observed_at_ns: at,
        no_mutation_observed: true,
        credential_value_present: false,
        order_submitted: false,
        cancellation_submitted: false,
        evidence_digest: [0; 32],
    }
    .sealed()
}

#[test]
#[allow(clippy::too_many_lines)]
fn complete_read_only_campaign_recovers_without_mutation_authority() {
    let mut owner = registered();
    sync(&mut owner, 1, 210, 80);
    owner
        .apply(&VenueCommand::ObserveParameters {
            command_id: VenueCommandId(id(84)),
            parameters: parameters(1, 214),
            recorded_at_ns: 214,
        })
        .unwrap();
    owner
        .apply(&VenueCommand::ObserveMode {
            command_id: VenueCommandId(id(85)),
            mode: mode(1, VenueMode::Normal, 215),
            recorded_at_ns: 215,
        })
        .unwrap();
    assert!(owner.snapshot(215).observation_ready);
    owner
        .apply(&VenueCommand::ObserveMode {
            command_id: VenueCommandId(id(86)),
            mode: mode(2, VenueMode::PostOnly, 216),
            recorded_at_ns: 216,
        })
        .unwrap();
    owner
        .apply(&VenueCommand::ObserveMode {
            command_id: VenueCommandId(id(87)),
            mode: mode(3, VenueMode::CancelOnly, 217),
            recorded_at_ns: 217,
        })
        .unwrap();
    owner
        .apply(&VenueCommand::ObserveRateLimit {
            command_id: VenueCommandId(id(88)),
            observation_id: id(89),
            backoff_ns: 50,
            automatic_retry_attempted: false,
            observed_at_ns: 218,
            recorded_at_ns: 218,
        })
        .unwrap();
    let restart = owner
        .apply(&VenueCommand::ObserveMode {
            command_id: VenueCommandId(id(90)),
            mode: mode(4, VenueMode::Restarting, 219),
            recorded_at_ns: 219,
        })
        .unwrap();
    let restart_req = requirement(restart);
    assert!(!owner.snapshot(219).observation_ready);
    let evidence = recovery(&restart_req, 2, 2, 5, 230, 91);
    owner
        .apply(&VenueCommand::Recover {
            command_id: VenueCommandId(id(92)),
            requirement: Box::new(restart_req),
            evidence: Box::new(evidence),
            recorded_at_ns: 230,
        })
        .unwrap();
    assert!(owner.snapshot(230).observation_ready);
    let failed = owner
        .apply(&VenueCommand::FailChannel {
            command_id: VenueCommandId(id(93)),
            channel: ChannelKind::AuthenticatedUser,
            failure_digest: id(94),
            observed_at_ns: 231,
            recorded_at_ns: 231,
        })
        .unwrap();
    let failed_req = requirement(failed);
    assert!(!owner.snapshot(231).observation_ready);
    let evidence = recovery(&failed_req, 3, 3, 6, 240, 95);
    owner
        .apply(&VenueCommand::Recover {
            command_id: VenueCommandId(id(96)),
            requirement: Box::new(failed_req),
            evidence: Box::new(evidence),
            recorded_at_ns: 240,
        })
        .unwrap();
    let outcome = owner
        .apply(&VenueCommand::Finalize {
            command_id: VenueCommandId(id(97)),
            report_id: id(98),
            finalized_at_ns: 241,
            recorded_at_ns: 241,
        })
        .unwrap();
    let report = match outcome.detail {
        VenueDetail::Finalized(v) => *v,
        _ => panic!("report"),
    };
    assert!(report.verify_digest());
    assert_eq!(report.covered_scenarios, VenueScenario::ALL);
    assert!(
        !report.live_environment_certified
            && !report.credential_material_created
            && !report.authenticated_session_opened
    );
    assert!(
        !report.order_endpoint_present
            && !report.cancel_endpoint_present
            && !report.order_submitted
            && !report.cancellation_submitted
    );
    assert!(
        !report.deployment_authority_granted
            && !report.trading_authority_granted
            && !report.submission_authority_granted
    );
}

#[test]
fn mutation_capable_contract_and_parameter_gap_halt() {
    let mut bad = plan();
    bad.authenticated_contract.order_endpoint_present = true;
    bad.authenticated_contract = bad.authenticated_contract.sealed();
    bad = bad.sealed(&policy());
    let mut owner = ReadOnlyVenueSupervisor::new(policy()).unwrap();
    assert_eq!(
        owner
            .apply(&VenueCommand::Register {
                command_id: VenueCommandId(id(100)),
                plan: Box::new(bad),
                recorded_at_ns: 200
            })
            .unwrap_err(),
        Error::Plan
    );
    let mut parameter_owner = registered();
    parameter_owner
        .apply(&VenueCommand::ObserveParameters {
            command_id: VenueCommandId(id(101)),
            parameters: parameters(1, 210),
            recorded_at_ns: 210,
        })
        .unwrap();
    assert_eq!(
        parameter_owner
            .apply(&VenueCommand::ObserveParameters {
                command_id: VenueCommandId(id(102)),
                parameters: parameters(3, 211),
                recorded_at_ns: 211
            })
            .unwrap_err(),
        Error::Parameters
    );
}

#[test]
fn one_healthy_channel_cannot_hide_user_failure_or_incomplete_recovery() {
    let mut owner = registered();
    sync(&mut owner, 1, 210, 110);
    owner
        .apply(&VenueCommand::ObserveParameters {
            command_id: VenueCommandId(id(114)),
            parameters: parameters(1, 214),
            recorded_at_ns: 214,
        })
        .unwrap();
    owner
        .apply(&VenueCommand::ObserveMode {
            command_id: VenueCommandId(id(115)),
            mode: mode(1, VenueMode::Normal, 215),
            recorded_at_ns: 215,
        })
        .unwrap();
    let failed = owner
        .apply(&VenueCommand::FailChannel {
            command_id: VenueCommandId(id(116)),
            channel: ChannelKind::AuthenticatedUser,
            failure_digest: id(117),
            observed_at_ns: 216,
            recorded_at_ns: 216,
        })
        .unwrap();
    let requirement = requirement(failed);
    let mut evidence = recovery(&requirement, 2, 2, 2, 220, 118);
    evidence.channel_snapshots.pop();
    evidence = evidence.sealed();
    assert_eq!(
        owner
            .apply(&VenueCommand::Recover {
                command_id: VenueCommandId(id(119)),
                requirement: Box::new(requirement),
                evidence: Box::new(evidence),
                recorded_at_ns: 220
            })
            .unwrap_err(),
        Error::Recovery
    );
    assert!(owner.is_halted());
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
    let recovery = VenueRecovery {
        owner: ReadOnlyVenueSupervisor::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableReadOnlyVenue::new(writer, recovery).unwrap();
    let command = VenueCommand::Register {
        command_id: VenueCommandId(id(150)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    };
    durable.apply(&command).unwrap();
    let expected = durable.owner().snapshot(200).digest;
    drop(durable);
    let checkpoint = VenueCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let checkpoint_path = dir.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&checkpoint_path).unwrap(), checkpoint);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).unwrap();
    assert_eq!(recovered.owner.snapshot(200).digest, expected);
    let report = VenueReport {
        report_id: id(151),
        plan_digest: id(152),
        security_report_digest: id(153),
        final_epoch: 3,
        final_parameter_version: 3,
        covered_scenarios: VenueScenario::ALL.to_vec(),
        finalized_at_ns: 300,
        status: VenueReportStatus::LocallyCertified,
        live_environment_certified: false,
        credential_material_created: false,
        authenticated_session_opened: false,
        order_endpoint_present: false,
        cancel_endpoint_present: false,
        order_submitted: false,
        cancellation_submitted: false,
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
    let recovery = VenueRecovery {
        owner: ReadOnlyVenueSupervisor::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableReadOnlyVenue::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&command),
        Err(VenueStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot(200).accepted_commands, 0);
    assert!(matches!(
        failing.apply(&command),
        Err(VenueStorageError::Halted(_))
    ));
}

proptest! { #[test] fn stale_channel_set_never_ready(age in 101_i64..10_000) { let mut owner = registered(); sync(&mut owner, 1, 210, 130); owner.apply(&VenueCommand::ObserveParameters { command_id: VenueCommandId(id(134)), parameters: parameters(1, 214), recorded_at_ns: 214 }).unwrap(); owner.apply(&VenueCommand::ObserveMode { command_id: VenueCommandId(id(135)), mode: mode(1, VenueMode::Normal, 215), recorded_at_ns: 215 }).unwrap(); prop_assert!(!owner.snapshot(215 + age).observation_ready); } }

#[test]
fn fixed_point_parameter_types_do_not_depend_on_financial_floats() {
    let p = parameters(1, 210);
    assert_eq!(p.tick_size_micros, 1_000);
    assert_eq!(p.taker_fee_bps, 200);
}
