use super::*;
use deployment_preflight::OperatorRole;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use tempfile::tempdir;

const PLAN_ID: [u8; 32] = [1; 32];
const ROLLBACK_PACKAGE: [u8; 32] = [6; 32];

fn id(value: u8) -> OrchestrationCommandId {
    OrchestrationCommandId([value; 32])
}

fn policy() -> OrchestrationPolicy {
    OrchestrationPolicy {
        maximum_waves: 4,
        maximum_regions: 4,
        maximum_preflight_age_ns: 500,
        maximum_plan_age_ns: 1_000,
        maximum_health_age_ns: 100,
        maximum_wave_duration_ns: 300,
    }
}

fn preflight() -> DeploymentPreflightReport {
    let mut report = DeploymentPreflightReport {
        report_id: [9; 32],
        deployment_package_id: [8; 32],
        package_digest: [7; 32],
        fleet_readiness_digest: [5; 32],
        fleet_governance_digest: [4; 32],
        package_expires_at_ns: 3_000,
        regions: vec!["eu-west".to_owned(), "us-east".to_owned()],
        rollback_package_digest: ROLLBACK_PACKAGE,
        evaluated_at_ns: 1_900,
        status: PreflightStatus::ReadyForManualDeployment,
        reasons: Vec::new(),
        approved_roles: vec![
            OperatorRole::Release,
            OperatorRole::Risk,
            OperatorRole::Operations,
        ],
        distinct_operator_count: 3,
        manual_operator_execution_required: true,
        credential_material_created: false,
        signing_authority_granted: false,
        deployment_authority_granted: false,
        rollback_execution_authority_granted: false,
        cloud_control_authority_granted: false,
        live_trading_authority_granted: false,
        report_digest: [0; 32],
    };
    report.report_digest = preflight_report_digest_for_test(&report);
    report
}

fn preflight_report_digest_for_test(value: &DeploymentPreflightReport) -> [u8; 32] {
    let mut clone = value.clone();
    clone.report_digest = [0; 32];
    digest_json(b"deployment-preflight-report-v1", &clone)
}

fn wave(value: u8, region: &str) -> DeploymentWave {
    DeploymentWave {
        wave_id: [value; 32],
        regions: vec![region.to_owned()],
        minimum_observation_ns: 50,
        maximum_duration_ns: 200,
        wave_digest: [0; 32],
    }
    .sealed()
}

fn plan() -> OrchestrationPlan {
    OrchestrationPlan {
        plan_id: PLAN_ID,
        created_at_ns: 2_000,
        expires_at_ns: 2_900,
        preflight: preflight(),
        waves: vec![wave(1, "eu-west"), wave(2, "us-east")],
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn health(sequence: u64, at: i64) -> RegionalHealthFrame {
    RegionalHealthFrame {
        sequence,
        observed_at_ns: at,
        valid_until_ns: at + 100,
        regions: vec![
            RegionHealth {
                region: "eu-west".to_owned(),
                package_applied: true,
                service_healthy: true,
                risk_healthy: true,
                reconciliation_healthy: true,
                capital_floor_preserved: true,
            },
            RegionHealth {
                region: "us-east".to_owned(),
                package_applied: true,
                service_healthy: true,
                risk_healthy: true,
                reconciliation_healthy: true,
                capital_floor_preserved: true,
            },
        ],
        source_digest: [3; 32],
        frame_digest: [0; 32],
    }
    .sealed()
}

fn register() -> OrchestrationCommand {
    OrchestrationCommand::Register {
        command_id: id(1),
        plan: Box::new(plan()),
        recorded_at_ns: 2_000,
    }
}

fn observe(command: u8, frame: RegionalHealthFrame) -> OrchestrationCommand {
    let at = frame.observed_at_ns;
    OrchestrationCommand::ObserveHealth {
        command_id: id(command),
        plan_id: PLAN_ID,
        frame,
        recorded_at_ns: at,
    }
}

fn start(command: u8, at: i64) -> OrchestrationCommand {
    OrchestrationCommand::Start {
        command_id: id(command),
        plan_id: PLAN_ID,
        recorded_at_ns: at,
    }
}

fn advance(command: u8, at: i64) -> OrchestrationCommand {
    OrchestrationCommand::Advance {
        command_id: id(command),
        plan_id: PLAN_ID,
        recorded_at_ns: at,
    }
}

fn rollback(command: u8, region: &str, at: i64) -> OrchestrationCommand {
    OrchestrationCommand::ObserveRollback {
        command_id: id(command),
        observation: RollbackObservation {
            observation_id: [command; 32],
            plan_id: PLAN_ID,
            region: region.to_owned(),
            rollback_package_digest: ROLLBACK_PACKAGE,
            baseline_restored: true,
            observed_at_ns: at,
            source_digest: [2; 32],
            observation_digest: [0; 32],
        }
        .sealed(),
        recorded_at_ns: at,
    }
}

fn successful_commands() -> Vec<OrchestrationCommand> {
    vec![
        register(),
        observe(2, health(0, 2_010)),
        start(3, 2_010),
        observe(4, health(1, 2_060)),
        advance(5, 2_060),
        observe(6, health(2, 2_110)),
        advance(7, 2_110),
        OrchestrationCommand::Finalize {
            command_id: id(8),
            plan_id: PLAN_ID,
            report_id: [8; 32],
            recorded_at_ns: 2_120,
        },
    ]
}

fn run(commands: &[OrchestrationCommand]) -> DeploymentOrchestrator {
    let mut owner = DeploymentOrchestrator::new(policy()).expect("owner");
    for command in commands {
        owner.apply(command).expect("valid command");
    }
    owner
}

#[test]
fn completes_two_health_gated_waves_without_authority() {
    let owner = run(&successful_commands());
    let snapshot = owner.snapshot();
    assert_eq!(snapshot.mode, Some(OrchestrationMode::Completed));
    let report = snapshot.last_report.expect("report");
    assert_eq!(report.status, OrchestrationReportStatus::SimulatedCompleted);
    assert_eq!(report.completed_wave_count, 2);
    assert!(report.manual_operator_execution_required);
    assert!(!report.credential_material_created);
    assert!(!report.deployment_authority_granted);
    assert!(!report.rollback_execution_authority_granted);
    assert!(!report.cloud_control_authority_granted);
    assert!(!report.live_trading_authority_granted);
    assert!(report.verify_digest());
}

#[test]
fn substituted_or_authority_bearing_preflight_halts_registration() {
    for mutate in [0_u8, 1] {
        let mut candidate = plan();
        if mutate == 0 {
            candidate.preflight.deployment_authority_granted = true;
        } else {
            candidate.preflight.report_digest[0] ^= 1;
        }
        candidate = candidate.sealed(&policy());
        let mut owner = DeploymentOrchestrator::new(policy()).expect("owner");
        assert!(matches!(
            owner.apply(&OrchestrationCommand::Register {
                command_id: id(20 + mutate),
                plan: Box::new(candidate),
                recorded_at_ns: 2_000,
            }),
            Err(Error::Plan)
        ));
        assert!(owner.is_halted());
    }
}

#[test]
fn duplicate_wave_identity_or_region_coverage_is_rejected() {
    for duplicate_id in [false, true] {
        let mut candidate = plan();
        if duplicate_id {
            candidate.waves[1] = wave(1, "us-east");
        } else {
            candidate.waves[1] = wave(2, "eu-west");
        }
        candidate = candidate.sealed(&policy());
        let mut owner = DeploymentOrchestrator::new(policy()).expect("owner");
        assert!(matches!(
            owner.apply(&OrchestrationCommand::Register {
                command_id: id(if duplicate_id { 30 } else { 31 }),
                plan: Box::new(candidate),
                recorded_at_ns: 2_000,
            }),
            Err(Error::Plan)
        ));
    }
}

#[test]
fn degradation_pauses_and_requires_explicit_healthy_resume() {
    let mut owner = run(&[register(), observe(2, health(0, 2_010)), start(3, 2_010)]);
    let mut degraded = health(1, 2_020);
    degraded.regions[0].service_healthy = false;
    degraded = degraded.sealed();
    let outcome = owner.apply(&observe(4, degraded)).expect("pause");
    assert_eq!(
        outcome.detail,
        OrchestrationDetail::Paused(PauseReason::HealthDegraded)
    );
    owner
        .apply(&observe(5, health(2, 2_030)))
        .expect("healthy observation");
    assert_eq!(owner.snapshot().mode, Some(OrchestrationMode::Paused));
    owner
        .apply(&OrchestrationCommand::OperatorResume {
            command_id: id(6),
            plan_id: PLAN_ID,
            operator_id: [9; 32],
            recorded_at_ns: 2_030,
        })
        .expect("explicit resume");
    assert_eq!(owner.snapshot().mode, Some(OrchestrationMode::Running));
}

#[test]
fn severe_failure_rolls_back_every_activated_region_in_reverse_order() {
    let mut owner = run(&[
        register(),
        observe(2, health(0, 2_010)),
        start(3, 2_010),
        observe(4, health(1, 2_060)),
        advance(5, 2_060),
    ]);
    let mut severe = health(2, 2_070);
    severe.regions[0].reconciliation_healthy = false;
    severe = severe.sealed();
    owner.apply(&observe(6, severe)).expect("latch rollback");
    assert_eq!(
        owner.snapshot().mode,
        Some(OrchestrationMode::RollbackRequired)
    );
    owner
        .apply(&rollback(7, "us-east", 2_080))
        .expect("second wave first");
    owner
        .apply(&rollback(8, "eu-west", 2_090))
        .expect("first wave second");
    assert_eq!(owner.snapshot().mode, Some(OrchestrationMode::RolledBack));
}

#[test]
fn wrong_rollback_order_halts_absorbingly() {
    let mut owner = run(&[
        register(),
        observe(2, health(0, 2_010)),
        start(3, 2_010),
        observe(4, health(1, 2_060)),
        advance(5, 2_060),
    ]);
    owner
        .apply(&OrchestrationCommand::OperatorAbort {
            command_id: id(6),
            plan_id: PLAN_ID,
            operator_id: [9; 32],
            reason_digest: [8; 32],
            recorded_at_ns: 2_070,
        })
        .expect("abort");
    assert!(matches!(
        owner.apply(&rollback(7, "eu-west", 2_080)),
        Err(Error::Rollback)
    ));
    assert!(matches!(
        owner.apply(&rollback(8, "us-east", 2_090)),
        Err(Error::Halted(_))
    ));
}

#[test]
fn abort_before_activation_needs_no_rollback() {
    let mut owner = run(&[register()]);
    let outcome = owner
        .apply(&OrchestrationCommand::OperatorAbort {
            command_id: id(2),
            plan_id: PLAN_ID,
            operator_id: [9; 32],
            reason_digest: [8; 32],
            recorded_at_ns: 2_001,
        })
        .expect("abort");
    assert_eq!(outcome.detail, OrchestrationDetail::Aborted);
    assert_eq!(owner.snapshot().mode, Some(OrchestrationMode::Aborted));
}

#[test]
fn restart_recovery_never_resumes_automatically() {
    let mut owner = run(&[register(), observe(2, health(0, 2_010)), start(3, 2_010)]);
    owner
        .apply(&OrchestrationCommand::Restart {
            command_id: id(4),
            plan_id: PLAN_ID,
            recorded_at_ns: 2_020,
        })
        .expect("restart");
    owner.apply(&observe(5, health(1, 2_030))).expect("health");
    owner
        .apply(&OrchestrationCommand::Recover {
            command_id: id(6),
            plan_id: PLAN_ID,
            recovery_epoch: 1,
            evidence_digest: [7; 32],
            recorded_at_ns: 2_030,
        })
        .expect("recover");
    assert_eq!(owner.snapshot().mode, Some(OrchestrationMode::Paused));
}

#[test]
fn wave_and_plan_timeout_latch_rollback() {
    let mut wave_owner = run(&[register(), observe(2, health(0, 2_010)), start(3, 2_010)]);
    let outcome = wave_owner
        .apply(&advance(4, 2_211))
        .expect("timeout transition");
    assert_eq!(
        outcome.detail,
        OrchestrationDetail::RollbackRequired(RollbackTrigger::WaveTimeout)
    );

    let mut plan_owner = run(&[register(), observe(2, health(0, 2_010)), start(3, 2_010)]);
    let outcome = plan_owner
        .apply(&advance(4, 2_901))
        .expect("plan timeout transition");
    assert_eq!(
        outcome.detail,
        OrchestrationDetail::RollbackRequired(RollbackTrigger::PlanTimeout)
    );
}

#[test]
fn canonical_report_detects_corruption_and_refuses_replacement() {
    let report = run(&successful_commands())
        .snapshot()
        .last_report
        .expect("report");
    let directory = tempdir().expect("directory");
    let path = directory.path().join("report.bin");
    write_report_create_new(&path, &report).expect("write");
    assert_eq!(read_report(&path).expect("read"), report);
    assert!(matches!(
        write_report_create_new(&path, &report),
        Err(OrchestrationReportFileError::Io(_))
    ));
    let mut bytes = std::fs::read(&path).expect("bytes");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, bytes).expect("corrupt");
    assert!(matches!(
        read_report(&path),
        Err(OrchestrationReportFileError::Checksum)
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
fn journal_replay_checkpoint_and_sync_failure_are_fail_closed() {
    let commands = successful_commands();
    let directory = tempdir().expect("directory");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 512 * 1024,
            max_segment_records: 3,
        },
    )
    .expect("writer");
    let recovery = OrchestrationRecovery {
        orchestrator: DeploymentOrchestrator::new(policy()).expect("owner"),
        last_sequence: None,
    };
    let mut durable = DurableOrchestrator::new(writer, recovery).expect("durable");
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let expected = durable.orchestrator().snapshot().digest;
    let checkpoint = OrchestrationCheckpoint {
        sequence: u64::try_from(commands.len() - 1).expect("sequence"),
        orchestrator_digest: expected,
    };
    let checkpoint_path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&checkpoint_path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.orchestrator.snapshot().digest, expected);

    let recovery = OrchestrationRecovery {
        orchestrator: DeploymentOrchestrator::new(policy()).expect("owner"),
        last_sequence: None,
    };
    let mut failing =
        DurableOrchestrator::new(FailingJournal::default(), recovery).expect("durable");
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(OrchestrationStorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(OrchestrationStorageError::Halted(_))
    ));
    assert_eq!(failing.orchestrator().snapshot().accepted_commands, 0);
}

#[test]
fn identical_commands_produce_identical_state_and_reports() {
    let commands = successful_commands();
    assert_eq!(run(&commands).snapshot(), run(&commands).snapshot());
}

proptest! {
    #[test]
    fn any_severe_active_region_health_latches_rollback(capital_ok in any::<bool>()) {
        let mut owner = run(&[register(), observe(2, health(0, 2_010)), start(3, 2_010)]);
        let mut frame = health(1, 2_020);
        frame.regions[0].capital_floor_preserved = capital_ok;
        frame.regions[0].reconciliation_healthy = false;
        frame = frame.sealed();
        owner.apply(&observe(4, frame)).expect("health transition");
        prop_assert_eq!(owner.snapshot().mode, Some(OrchestrationMode::RollbackRequired));
    }
}
