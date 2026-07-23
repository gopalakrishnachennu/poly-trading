use super::*;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(v: u8) -> [u8; 32] {
    [v; 32]
}
fn policy() -> CampaignPolicy {
    CampaignPolicy {
        maximum_chain_report_age_ns: 10_000,
        maximum_plan_lifetime_ns: 5_000,
        maximum_tick_age_ns: 100,
        minimum_accelerated_duration_ns: 10_800_000_000_000,
        minimum_rollovers: 2,
        maximum_queue_depth: 100,
        maximum_memory_bytes: 1_000_000,
        maximum_open_files: 100,
        maximum_journal_bytes: 10_000_000,
        maximum_latency_ns: 1_000_000,
    }
}
fn upstream() -> ChainReport {
    ChainReport {
        report_id: id(1),
        plan_digest: id(2),
        venue_report_digest: id(3),
        final_frame_digest: id(4),
        final_finalized_number: 100,
        covered_scenarios: ChainScenario::ALL.to_vec(),
        finalized_at_ns: 100,
        status: ChainReportStatus::LocallyCertified,
        live_environment_certified: false,
        rpc_connection_opened: false,
        credential_material_created: false,
        wallet_access_granted: false,
        signature_produced: false,
        transaction_submitted: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        submission_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed()
}
fn subjects() -> RuntimeSubjects {
    RuntimeSubjects {
        artifact_digest: id(10),
        configuration_digest: id(11),
        venue_runtime_digest: id(12),
        chain_runtime_digest: id(13),
        checkpoint_schema_digest: id(14),
        subjects_digest: [0; 32],
    }
    .sealed()
}
fn plan() -> CampaignPlan {
    CampaignPlan {
        plan_id: id(15),
        chain_report: upstream(),
        subjects: subjects(),
        required_scenarios: CampaignScenario::ALL.to_vec(),
        created_at_ns: 200,
        expires_at_ns: 4_000,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}
fn resources() -> ResourceSample {
    ResourceSample {
        queue_depth: 10,
        memory_bytes: 100_000,
        open_files: 10,
        journal_bytes: 1_000_000,
        maximum_latency_ns: 100_000,
    }
}
fn tick(b: u8, sequence: u64, hour: u64, logical: i64, at: i64) -> CampaignTick {
    CampaignTick {
        tick_id: id(b),
        sequence,
        hour_index: hour,
        logical_time_ns: logical,
        event_time_ns: at - 2,
        received_time_ns: at - 1,
        observed_at_ns: at,
        venue_state_digest: id(20),
        chain_state_digest: id(21),
        resources: resources(),
        healthy: true,
        tick_digest: [0; 32],
    }
    .sealed()
}
fn fixture(b: u8, scenario: CampaignScenario) -> IntegrityFixture {
    IntegrityFixture {
        fixture_id: id(b),
        scenario,
        trigger_digest: id(b + 1),
        isolated: true,
        halted: true,
        state_contribution: false,
        fixture_digest: [0; 32],
    }
    .sealed()
}
fn registered() -> ContinuousCampaign {
    let mut owner = ContinuousCampaign::new(policy()).unwrap();
    owner
        .apply(&CampaignCommand::Register {
            command_id: CampaignCommandId(id(30)),
            plan: Box::new(plan()),
            recorded_at_ns: 200,
        })
        .unwrap();
    owner
}
fn requirement(outcome: CampaignOutcome) -> RecoveryRequirement {
    match outcome.detail {
        CampaignDetail::RecoveryRequired(v) => *v,
        _ => panic!("requirement"),
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn accelerated_campaign_covers_failures_without_claiming_real_soak() {
    let mut owner = registered();
    owner
        .apply(&CampaignCommand::ObserveTick {
            command_id: CampaignCommandId(id(31)),
            tick: tick(40, 1, 0, 0, 210),
            recorded_at_ns: 210,
        })
        .unwrap();
    owner
        .apply(&CampaignCommand::ObserveTick {
            command_id: CampaignCommandId(id(32)),
            tick: tick(41, 2, 1, 3_600_000_000_000, 211),
            recorded_at_ns: 211,
        })
        .unwrap();
    let mut sequence = 2;
    for (offset, kind) in [
        DisruptionKind::CheckpointRestart,
        DisruptionKind::VenuePartition,
        DisruptionKind::ChainPartition,
        DisruptionKind::DeadMan,
    ]
    .into_iter()
    .enumerate()
    {
        let b = 50 + u8::try_from(offset * 10).unwrap();
        let result = owner
            .apply(&CampaignCommand::Disrupt {
                command_id: CampaignCommandId(id(b)),
                requirement_id: id(b + 1),
                kind,
                checkpoint_digest: id(90),
                trigger_digest: id(b + 2),
                recorded_at_ns: 220 + i64::try_from(offset * 2).unwrap(),
            })
            .unwrap();
        let req = requirement(result);
        sequence += 1;
        let recovery_tick = tick(
            b + 3,
            sequence,
            1,
            3_600_000_000_000 + i64::try_from(offset + 1).unwrap(),
            221 + i64::try_from(offset * 2).unwrap(),
        );
        let evidence = RecoveryEvidence {
            evidence_id: id(b + 3),
            requirement_digest: req.requirement_digest,
            checkpoint_digest: req.checkpoint_digest,
            tick: recovery_tick,
            no_mutation_observed: true,
            credential_present: false,
            connection_opened: false,
            wallet_action_observed: false,
            evidence_digest: [0; 32],
        }
        .sealed();
        owner
            .apply(&CampaignCommand::Recover {
                command_id: CampaignCommandId(id(b + 4)),
                requirement: Box::new(req),
                evidence: Box::new(evidence),
                recorded_at_ns: 221 + i64::try_from(offset * 2).unwrap(),
            })
            .unwrap();
    }
    owner
        .apply(&CampaignCommand::ObserveTick {
            command_id: CampaignCommandId(id(100)),
            tick: tick(101, sequence + 1, 2, 10_800_000_000_000, 235),
            recorded_at_ns: 235,
        })
        .unwrap();
    owner
        .apply(&CampaignCommand::RecordIntegrityFixture {
            command_id: CampaignCommandId(id(102)),
            fixture: fixture(103, CampaignScenario::ClockRegression),
            recorded_at_ns: 236,
        })
        .unwrap();
    owner
        .apply(&CampaignCommand::RecordIntegrityFixture {
            command_id: CampaignCommandId(id(104)),
            fixture: fixture(105, CampaignScenario::DurableCorruption),
            recorded_at_ns: 237,
        })
        .unwrap();
    let result = owner
        .apply(&CampaignCommand::Finalize {
            command_id: CampaignCommandId(id(106)),
            report_id: id(107),
            operations_operator_digest: id(108),
            risk_operator_digest: id(109),
            real_elapsed_duration_ns: 1_000,
            finalized_at_ns: 238,
            recorded_at_ns: 238,
        })
        .unwrap();
    let report = match result.detail {
        CampaignDetail::Finalized(v) => *v,
        _ => panic!("report"),
    };
    assert_eq!(report.covered_scenarios, CampaignScenario::ALL);
    assert_eq!(report.rollover_count, 2);
    assert_eq!(report.accelerated_duration_ns, 10_800_000_000_000);
    assert_eq!(report.real_elapsed_duration_ns, 1_000);
    assert!(
        !report.real_multi_day_environment_certified
            && !report.credential_material_created
            && !report.external_connection_opened
            && !report.external_mutation_observed
            && !report.deployment_authority_granted
            && !report.trading_authority_granted
            && !report.submission_authority_granted
    );
}

#[test]
fn chronology_regression_and_bad_recovery_halt() {
    let mut owner = registered();
    owner
        .apply(&CampaignCommand::ObserveTick {
            command_id: CampaignCommandId(id(80)),
            tick: tick(81, 1, 0, 10, 210),
            recorded_at_ns: 210,
        })
        .unwrap();
    assert_eq!(
        owner
            .apply(&CampaignCommand::ObserveTick {
                command_id: CampaignCommandId(id(82)),
                tick: tick(83, 2, 0, 9, 211),
                recorded_at_ns: 211
            })
            .unwrap_err(),
        Error::Tick
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
    let recovery = CampaignRecovery {
        owner: ContinuousCampaign::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableCampaign::new(writer, recovery).unwrap();
    let command = CampaignCommand::Register {
        command_id: CampaignCommandId(id(90)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    };
    durable.apply(&command).unwrap();
    let expected = durable.owner().snapshot(200).digest;
    drop(durable);
    let checkpoint = CampaignCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let checkpoint_path = dir.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&checkpoint_path).unwrap(), checkpoint);
    assert_eq!(
        recover_segmented(&segments, policy(), Some(checkpoint))
            .unwrap()
            .owner
            .snapshot(200)
            .digest,
        expected
    );
    let report = CampaignReport {
        report_id: id(91),
        plan_digest: id(92),
        chain_report_digest: id(93),
        final_tick_digest: id(94),
        accelerated_duration_ns: 10,
        real_elapsed_duration_ns: 1,
        rollover_count: 2,
        covered_scenarios: CampaignScenario::ALL.to_vec(),
        operations_operator_digest: id(95),
        risk_operator_digest: id(96),
        finalized_at_ns: 300,
        status: CampaignReportStatus::LocallyCertified,
        real_multi_day_environment_certified: false,
        credential_material_created: false,
        external_connection_opened: false,
        external_mutation_observed: false,
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
    let recovery = CampaignRecovery {
        owner: ContinuousCampaign::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableCampaign::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&command),
        Err(CampaignStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot(200).accepted_commands, 0);
    assert!(matches!(
        failing.apply(&command),
        Err(CampaignStorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn any_individual_resource_excess_never_accepts(extra in 1_u64..100_000, dimension in 0_u8..5) {
        let mut owner = registered(); let mut value = tick(100, 1, 0, 0, 210);
        match dimension { 0 => value.resources.queue_depth = policy().maximum_queue_depth + extra, 1 => value.resources.memory_bytes = policy().maximum_memory_bytes + extra, 2 => value.resources.open_files = policy().maximum_open_files + extra, 3 => value.resources.journal_bytes = policy().maximum_journal_bytes + extra, _ => value.resources.maximum_latency_ns = policy().maximum_latency_ns + i64::try_from(extra).unwrap() }
        value = value.sealed(); prop_assert_eq!(owner.apply(&CampaignCommand::ObserveTick { command_id: CampaignCommandId(id(101)), tick: value, recorded_at_ns: 210 }).unwrap_err(), Error::Tick);
    }
}
