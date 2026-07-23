use super::*;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use tempfile::tempdir;

const CREATED: i64 = 2_000;

const fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> FleetPolicy {
    FleetPolicy {
        maximum_evidence: 16,
        maximum_regions: 8,
        minimum_regions: 2,
        minimum_abort_drills: 1,
        minimum_rollback_drills: 2,
        maximum_report_age_ns: 1_000,
        maximum_campaign_age_ns: 1_000,
    }
}

fn seal_report(mut report: RolloutReport) -> RolloutReport {
    report.report_digest = [0; 32];
    let serialized = serde_json::to_vec(&report).expect("report JSON");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"canary-rollout-report-v1");
    hasher.update(&(serialized.len() as u64).to_le_bytes());
    hasher.update(&serialized);
    report.report_digest = *hasher.finalize().as_bytes();
    report
}

fn report(id: u8, status: RolloutReportStatus, trigger: Option<RollbackTrigger>) -> RolloutReport {
    seal_report(RolloutReport {
        report_id: bytes(id),
        plan_id: bytes(id + 20),
        plan_digest: bytes(id + 40),
        eligibility_record_digest: bytes(1),
        artifacts_digest: bytes(2),
        rollback_digest: bytes(3),
        finalized_at_ns: 1_950 + i64::from(id),
        status,
        completed_stage_count: 1,
        final_stage_index: Some(0),
        final_target_bps: 100,
        health_frame_count: 4,
        pause_count: 0,
        restart_count: 0,
        recovery_epoch: 0,
        rollback_trigger: trigger,
        abort_operator_id: None,
        operator_execution_required: true,
        rollout_execution_authority_granted: false,
        rollback_execution_authority_granted: false,
        deployment_authority_granted: false,
        credential_authority_granted: false,
        live_trading_authority_granted: false,
        report_digest: [0; 32],
    })
}

fn evidence(id: u8, region: &str, report: RolloutReport) -> RegionalEvidence {
    RegionalEvidence {
        evidence_id: bytes(id),
        region: region.to_owned(),
        environment_digest: bytes(id + 100),
        report,
    }
}

fn manifest() -> FleetManifest {
    let mut abort = report(12, RolloutReportStatus::OperatorAborted, None);
    abort.abort_operator_id = Some(bytes(90));
    abort.final_stage_index = None;
    abort = seal_report(abort);
    let evidence = vec![
        evidence(
            1,
            "us-east",
            report(10, RolloutReportStatus::SimulatedCompleted, None),
        ),
        evidence(
            2,
            "eu-west",
            report(11, RolloutReportStatus::SimulatedCompleted, None),
        ),
        evidence(3, "us-east", abort),
        evidence(
            4,
            "us-east",
            report(
                13,
                RolloutReportStatus::RollbackRequired,
                Some(RollbackTrigger::CapitalFloorBreach),
            ),
        ),
        evidence(
            5,
            "eu-west",
            report(
                14,
                RolloutReportStatus::RollbackRequired,
                Some(RollbackTrigger::ReconciliationTimeout),
            ),
        ),
    ];
    FleetManifest {
        campaign_id: bytes(4),
        created_at_ns: CREATED,
        expires_at_ns: 2_900,
        release_digest: bytes(1),
        artifacts_digest: bytes(2),
        rollback_digest: bytes(3),
        required_regions: vec!["us-east".to_owned(), "eu-west".to_owned()],
        required_rollback_triggers: vec![
            RollbackTrigger::CapitalFloorBreach,
            RollbackTrigger::ReconciliationTimeout,
        ],
        freeze: ChangeFreeze {
            freeze_id: bytes(5),
            release_digest: bytes(1),
            artifacts_digest: bytes(2),
            starts_at_ns: 1_900,
            ends_at_ns: 3_000,
            emergency_change_forbidden: true,
            freeze_digest: [0; 32],
        }
        .sealed(),
        evidence,
        policy_digest: [0; 32],
        evidence_set_digest: [0; 32],
        manifest_digest: [0; 32],
    }
    .sealed(&policy())
}

fn register(manifest: FleetManifest) -> FleetCommand {
    FleetCommand::Register {
        command_id: FleetCommandId(bytes(30)),
        manifest: Box::new(manifest),
        recorded_at_ns: CREATED,
    }
}

fn finalize(at: i64) -> FleetCommand {
    FleetCommand::Finalize {
        command_id: FleetCommandId(bytes(31)),
        campaign_id: bytes(4),
        dossier_id: bytes(32),
        evaluated_at_ns: at,
        recorded_at_ns: at,
    }
}

fn commands() -> Vec<FleetCommand> {
    vec![register(manifest()), finalize(2_100)]
}

fn run(commands: &[FleetCommand]) -> FleetRolloutGovernance {
    let mut governance = FleetRolloutGovernance::new(policy()).expect("governance");
    for command in commands {
        governance.apply(command).expect("command");
    }
    governance
}

#[test]
fn complete_diverse_campaign_is_ready_without_any_external_authority() {
    let governance = run(&commands());
    let dossier = governance.snapshot().last_dossier.expect("dossier");
    assert_eq!(dossier.status, DossierStatus::OperationallyReady);
    assert!(dossier.reasons.is_empty());
    assert_eq!(
        dossier.aggregate.completed_regions,
        vec!["eu-west", "us-east"]
    );
    assert_eq!(dossier.aggregate.abort_drill_count, 1);
    assert_eq!(dossier.aggregate.rollback_drill_count, 2);
    assert!(dossier.operator_execution_required);
    assert!(!dossier.fleet_execution_authority_granted);
    assert!(!dossier.deployment_authority_granted);
    assert!(!dossier.rollback_execution_authority_granted);
    assert!(!dossier.credential_authority_granted);
    assert!(!dossier.live_trading_authority_granted);
    assert!(dossier.verify_digest());
    let current = governance
        .current_readiness(2_101)
        .expect("current readiness");
    assert_eq!(current.dossier_digest, dossier.dossier_digest);
    assert_eq!(
        current.completed_regions,
        dossier.aggregate.completed_regions
    );
    assert!(current.verify_digest());
}

#[test]
fn missing_region_drill_and_trigger_are_independently_attributed() {
    let mut value = manifest();
    value.evidence.retain(|item| {
        item.region != "eu-west" || item.report.status == RolloutReportStatus::OperatorAborted
    });
    value = value.sealed(&policy());
    let governance = run(&[register(value), finalize(2_100)]);
    let reasons = governance.snapshot().last_dossier.expect("dossier").reasons;
    assert!(reasons.contains(&ReadinessReason::MissingRegionCompletion(
        "eu-west".to_owned()
    )));
    assert!(reasons.contains(&ReadinessReason::InsufficientRollbackDrills));
    assert!(reasons.contains(&ReadinessReason::MissingRollbackTrigger(
        RollbackTrigger::ReconciliationTimeout
    )));
}

#[test]
fn duplicate_plan_cannot_inflate_evidence() {
    let mut value = manifest();
    let first_plan = value.evidence[0].report.plan_digest;
    value.evidence[1].report.plan_digest = first_plan;
    value.evidence[1].report = seal_report(value.evidence[1].report.clone());
    value = value.sealed(&policy());
    let governance = run(&[register(value), finalize(2_100)]);
    let dossier = governance.snapshot().last_dossier.expect("dossier");
    assert!(dossier
        .reasons
        .contains(&ReadinessReason::DuplicateEvidence));
    assert_eq!(dossier.aggregate.unique_plan_count, 4);
}

#[test]
fn stale_report_is_excluded_and_attributed() {
    let mut value = manifest();
    let report_id = value.evidence[0].report.report_id;
    value.evidence[0].report.finalized_at_ns = 999;
    value.evidence[0].report = seal_report(value.evidence[0].report.clone());
    value = value.sealed(&policy());
    let governance = run(&[register(value), finalize(2_100)]);
    let reasons = governance.snapshot().last_dossier.expect("dossier").reasons;
    assert!(reasons.contains(&ReadinessReason::StaleReport(report_id)));
}

#[test]
fn freeze_end_is_exclusive_and_campaign_expiry_is_attributable() {
    let mut value = manifest();
    value.expires_at_ns = 3_000;
    value = value.sealed(&policy());
    let governance = run(&[register(value), finalize(3_000)]);
    let dossier = governance.snapshot().last_dossier.expect("dossier");
    assert!(dossier
        .reasons
        .contains(&ReadinessReason::ChangeFreezeInactive));

    let governance = run(&[register(manifest()), finalize(2_901)]);
    assert!(governance
        .snapshot()
        .last_dossier
        .expect("dossier")
        .reasons
        .contains(&ReadinessReason::CampaignExpired));
}

#[test]
fn revocation_is_exact_subject_irreversible_and_denies_readiness() {
    let mut governance = FleetRolloutGovernance::new(policy()).expect("governance");
    governance.apply(&register(manifest())).expect("register");
    governance.apply(&finalize(2_025)).expect("ready dossier");
    assert_eq!(
        governance
            .snapshot()
            .last_dossier
            .expect("ready dossier")
            .status,
        DossierStatus::OperationallyReady
    );
    let revocation = RevocationRecord {
        revocation_id: bytes(40),
        release_digest: bytes(1),
        artifacts_digest: bytes(2),
        operator_id: bytes(41),
        reason_digest: bytes(42),
        effective_at_ns: 2_050,
        revocation_digest: [0; 32],
    }
    .sealed();
    governance
        .apply(&FleetCommand::Revoke {
            command_id: FleetCommandId(bytes(43)),
            campaign_id: bytes(4),
            revocation,
            recorded_at_ns: 2_050,
        })
        .expect("revoke");
    assert!(governance.snapshot().last_dossier.is_none());
    assert!(matches!(
        governance.current_readiness(2_051),
        Err(Error::Dossier)
    ));
    let mut new_finalize = finalize(2_100);
    if let FleetCommand::Finalize {
        command_id,
        dossier_id,
        ..
    } = &mut new_finalize
    {
        *command_id = FleetCommandId(bytes(44));
        *dossier_id = bytes(45);
    }
    governance.apply(&new_finalize).expect("revoked dossier");
    let dossier = governance.snapshot().last_dossier.expect("dossier");
    assert_eq!(dossier.status, DossierStatus::NotReady);
    assert!(dossier.reasons.contains(&ReadinessReason::ReleaseRevoked));
}

#[test]
fn artifact_substitution_and_noncanonical_evidence_order_halt() {
    let mut substituted = manifest();
    substituted.evidence[0].report.artifacts_digest = bytes(99);
    substituted.evidence[0].report = seal_report(substituted.evidence[0].report.clone());
    substituted = substituted.sealed(&policy());
    let mut governance = FleetRolloutGovernance::new(policy()).expect("governance");
    assert!(matches!(
        governance.apply(&register(substituted)),
        Err(Error::Evidence)
    ));
    assert!(governance.is_halted());

    let mut reordered = manifest();
    reordered.evidence.swap(0, 1);
    reordered.evidence_set_digest = fleet_evidence_digest(&reordered.evidence);
    reordered.manifest_digest = manifest_digest(&reordered);
    let mut governance = FleetRolloutGovernance::new(policy()).expect("governance");
    assert!(matches!(
        governance.apply(&register(reordered)),
        Err(Error::Manifest)
    ));
}

#[test]
fn canonical_dossier_file_is_create_new_and_corruption_detecting() {
    let dossier = run(&commands()).snapshot().last_dossier.expect("dossier");
    let directory = tempdir().expect("dir");
    let path = directory.path().join("dossier.bin");
    write_dossier_create_new(&path, &dossier).expect("write");
    assert_eq!(read_dossier(&path).expect("read"), dossier);
    assert!(write_dossier_create_new(&path, &dossier).is_err());

    let noncanonical_path = directory.path().join("noncanonical.bin");
    let mut noncanonical = std::fs::read(&path).expect("bytes");
    noncanonical.truncate(noncanonical.len() - 32);
    noncanonical.insert(24, b' ');
    let body_len = u64::try_from(noncanonical.len() - 24).expect("body length");
    noncanonical[16..24].copy_from_slice(&body_len.to_le_bytes());
    let checksum = blake3::hash(&noncanonical);
    noncanonical.extend_from_slice(checksum.as_bytes());
    std::fs::write(&noncanonical_path, noncanonical).expect("noncanonical fixture");
    assert!(matches!(
        read_dossier(&noncanonical_path),
        Err(DossierFileError::NonCanonical)
    ));

    let mut data = std::fs::read(&path).expect("bytes");
    data[30] ^= 1;
    std::fs::write(&path, data).expect("corrupt test fixture");
    assert!(matches!(
        read_dossier(&path),
        Err(DossierFileError::Checksum)
    ));
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
    let commands = commands();
    let directory = tempdir().expect("dir");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 512 * 1024,
            max_segment_records: 1,
        },
    )
    .expect("writer");
    let recovery = FleetRecovery {
        governance: FleetRolloutGovernance::new(policy()).expect("governance"),
        last_sequence: None,
    };
    let mut durable = DurableFleetGovernance::new(writer, recovery).expect("durable");
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let expected = durable.governance().snapshot().digest;
    let checkpoint = FleetCheckpoint {
        sequence: u64::try_from(commands.len() - 1).expect("sequence"),
        governance_digest: expected,
    };
    let checkpoint_path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&checkpoint_path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.governance.snapshot().digest, expected);

    let recovery = FleetRecovery {
        governance: FleetRolloutGovernance::new(policy()).expect("governance"),
        last_sequence: None,
    };
    let mut failing =
        DurableFleetGovernance::new(FailingJournal::default(), recovery).expect("new");
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(FleetStorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(FleetStorageError::Halted(_))
    ));
    assert_eq!(failing.governance().snapshot().accepted_commands, 0);
}

#[test]
fn identical_commands_produce_identical_complete_state() {
    let first = run(&commands());
    let second = run(&commands());
    assert_eq!(first.snapshot().digest, second.snapshot().digest);
    assert_eq!(
        first.snapshot().last_dossier,
        second.snapshot().last_dossier
    );
}

proptest! {
    #[test]
    fn duplicate_plan_never_counts_as_an_additional_plan(index in 1_usize..5) {
        let mut value = manifest();
        value.evidence[index].report.plan_digest = value.evidence[0].report.plan_digest;
        value.evidence[index].report = seal_report(value.evidence[index].report.clone());
        value = value.sealed(&policy());
        let mut governance = FleetRolloutGovernance::new(policy()).expect("governance");
        let outcome = governance.apply(&register(value)).expect("register");
        let FleetDetail::Registered { aggregate, reasons } = outcome.detail else {
            prop_assert!(false, "registered detail");
            return Ok(());
        };
        prop_assert_eq!(aggregate.unique_plan_count, 4);
        prop_assert!(reasons.contains(&ReadinessReason::DuplicateEvidence));
    }
}
