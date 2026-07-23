use super::*;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use promotion_governance::CandidateAggregate;
use proptest::prelude::*;
use tempfile::tempdir;

const CREATED: i64 = 1_000;
const START: i64 = 1_100;

const fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> SimulatorPolicy {
    SimulatorPolicy {
        maximum_windows: 4,
        maximum_stages: 4,
        maximum_health_age_ns: 200,
        maximum_plan_age_ns: 1_000,
        maximum_target_bps: 1_000,
    }
}

fn rollback() -> RollbackCriteria {
    RollbackCriteria {
        criteria_id: bytes(1),
        rollback_target_digest: bytes(2),
        maximum_canary_duration_ns: 900,
        maximum_unreconciled_ns: 10,
        maximum_unknown_state_ns: 10,
        maximum_session_loss_micros: 100,
        maximum_consecutive_faults: 2,
        require_capital_floor_halt: true,
        require_reconciliation_halt: true,
        criteria_digest: [0; 32],
    }
    .sealed()
}

fn seal_record(mut record: CanaryEligibilityRecord) -> CanaryEligibilityRecord {
    record.record_digest = [0; 32];
    let serialized = serde_json::to_vec(&record).expect("record JSON");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"promotion-canary-record-v1");
    hasher.update(&(serialized.len() as u64).to_le_bytes());
    hasher.update(&serialized);
    record.record_digest = *hasher.finalize().as_bytes();
    record
}

fn eligibility() -> CanaryEligibilityRecord {
    let rollback = rollback();
    seal_record(CanaryEligibilityRecord {
        record_id: bytes(3),
        candidate_id: bytes(4),
        candidate_digest: bytes(5),
        evidence_set_digest: bytes(6),
        baseline_digest: bytes(7),
        artifacts_digest: bytes(8),
        rollback_digest: rollback.criteria_digest,
        policy_digest: bytes(9),
        evaluated_at_ns: CREATED - 10,
        valid_until_ns: 2_000,
        status: CanaryStatus::CanaryEligible,
        reasons: Vec::new(),
        aggregate: CandidateAggregate {
            unique_campaigns: 3,
            distinct_manifests: 3,
            distinct_schedules: 3,
            distinct_final_states: 3,
            total_sessions: 6,
            total_steps: 30,
            total_fault_cycles: 9,
        },
        risk_decision_digest: Some(bytes(10)),
        release_decision_digest: Some(bytes(11)),
        dual_control_complete: true,
        operator_execution_required: true,
        rollback_required_on_threshold: true,
        canary_execution_authority_granted: false,
        promotion_authority_granted: false,
        deployment_authority_granted: false,
        credential_authority_granted: false,
        live_trading_authority_granted: false,
        record_digest: [0; 32],
    })
}

fn plan() -> RolloutPlan {
    RolloutPlan {
        plan_id: bytes(12),
        created_at_ns: CREATED,
        scheduled_start_ns: START,
        scheduled_end_ns: 1_900,
        eligibility: eligibility(),
        rollback: rollback(),
        windows: vec![
            MaintenanceWindow {
                window_id: bytes(13),
                start_ns: START,
                end_ns: 1_600,
            },
            MaintenanceWindow {
                window_id: bytes(14),
                start_ns: 1_700,
                end_ns: 1_900,
            },
        ],
        stages: vec![
            RolloutStage {
                stage_id: bytes(15),
                target_bps: 100,
                minimum_observation_ns: 50,
                maximum_stage_ns: 300,
            },
            RolloutStage {
                stage_id: bytes(16),
                target_bps: 500,
                minimum_observation_ns: 50,
                maximum_stage_ns: 300,
            },
            RolloutStage {
                stage_id: bytes(17),
                target_bps: 1_000,
                minimum_observation_ns: 50,
                maximum_stage_ns: 300,
            },
        ],
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn health(sequence: u64, at: i64) -> HealthFrame {
    HealthFrame {
        sequence,
        observed_at_ns: at,
        valid_until_ns: at + 100,
        strategy_healthy: true,
        risk_healthy: true,
        market_feed_healthy: true,
        user_feed_healthy: true,
        reconciliation_healthy: true,
        capital_floor_preserved: true,
        unreconciled_age_ns: 0,
        unknown_state_age_ns: 0,
        session_loss_micros: 0,
        consecutive_faults: 0,
        source_digest: bytes(u8::try_from(sequence + 30).expect("source")),
        frame_digest: [0; 32],
    }
    .sealed()
}

fn register_command() -> RolloutCommand {
    RolloutCommand::RegisterPlan {
        command_id: RolloutCommandId(bytes(20)),
        plan: Box::new(plan()),
        recorded_at_ns: CREATED,
    }
}

fn observe_command(id: u8, frame: HealthFrame) -> RolloutCommand {
    RolloutCommand::ObserveHealth {
        command_id: RolloutCommandId(bytes(id)),
        plan_id: plan().plan_id,
        recorded_at_ns: frame.observed_at_ns,
        frame,
    }
}

fn successful_commands() -> Vec<RolloutCommand> {
    let plan = plan();
    vec![
        register_command(),
        observe_command(21, health(1, START)),
        RolloutCommand::Start {
            command_id: RolloutCommandId(bytes(22)),
            plan_id: plan.plan_id,
            recorded_at_ns: START,
        },
        observe_command(23, health(2, START + 50)),
        RolloutCommand::Advance {
            command_id: RolloutCommandId(bytes(24)),
            plan_id: plan.plan_id,
            recorded_at_ns: START + 50,
        },
        observe_command(25, health(3, START + 100)),
        RolloutCommand::Advance {
            command_id: RolloutCommandId(bytes(26)),
            plan_id: plan.plan_id,
            recorded_at_ns: START + 100,
        },
        observe_command(27, health(4, START + 150)),
        RolloutCommand::Advance {
            command_id: RolloutCommandId(bytes(28)),
            plan_id: plan.plan_id,
            recorded_at_ns: START + 150,
        },
        RolloutCommand::Finalize {
            command_id: RolloutCommandId(bytes(29)),
            plan_id: plan.plan_id,
            report_id: bytes(30),
            recorded_at_ns: START + 160,
        },
    ]
}

fn run(commands: &[RolloutCommand]) -> CanaryRolloutSimulator {
    let mut simulator = CanaryRolloutSimulator::new(policy()).expect("simulator");
    for command in commands {
        simulator.apply(command).expect("command");
    }
    simulator
}

#[test]
fn ordered_healthy_rollout_completes_without_granting_authority() {
    let simulator = run(&successful_commands());
    let report = simulator.snapshot().last_report.expect("report");
    assert_eq!(report.status, RolloutReportStatus::SimulatedCompleted);
    assert_eq!(report.completed_stage_count, 3);
    assert_eq!(report.final_target_bps, 1_000);
    assert!(report.operator_execution_required);
    assert!(!report.rollout_execution_authority_granted);
    assert!(!report.rollback_execution_authority_granted);
    assert!(!report.deployment_authority_granted);
    assert!(!report.credential_authority_granted);
    assert!(!report.live_trading_authority_granted);
    assert!(report.verify_digest());
}

#[test]
fn start_outside_maintenance_window_fails_closed() {
    let mut simulator = CanaryRolloutSimulator::new(policy()).expect("simulator");
    simulator.apply(&register_command()).expect("register");
    simulator
        .apply(&observe_command(31, health(1, 1_650)))
        .expect("health");
    assert!(matches!(
        simulator.apply(&RolloutCommand::Start {
            command_id: RolloutCommandId(bytes(32)),
            plan_id: plan().plan_id,
            recorded_at_ns: 1_650,
        }),
        Err(Error::GateClosed)
    ));
    assert!(simulator.is_halted());
}

#[test]
fn unhealthy_frame_pauses_and_healthy_frame_does_not_resume_automatically() {
    let plan = plan();
    let mut simulator = CanaryRolloutSimulator::new(policy()).expect("simulator");
    simulator.apply(&register_command()).expect("register");
    simulator
        .apply(&observe_command(33, health(1, START)))
        .expect("health");
    simulator
        .apply(&RolloutCommand::Start {
            command_id: RolloutCommandId(bytes(34)),
            plan_id: plan.plan_id,
            recorded_at_ns: START,
        })
        .expect("start");
    let mut unhealthy = health(2, START + 10);
    unhealthy.market_feed_healthy = false;
    unhealthy = unhealthy.sealed();
    simulator
        .apply(&observe_command(35, unhealthy))
        .expect("unhealthy");
    assert_eq!(simulator.snapshot().mode, Some(RolloutMode::Paused));
    simulator
        .apply(&observe_command(36, health(3, START + 20)))
        .expect("healthy");
    assert_eq!(simulator.snapshot().mode, Some(RolloutMode::Paused));
    simulator
        .apply(&RolloutCommand::OperatorResume {
            command_id: RolloutCommandId(bytes(37)),
            plan_id: plan.plan_id,
            operator_id: bytes(90),
            recorded_at_ns: START + 21,
        })
        .expect("resume");
    assert_eq!(simulator.snapshot().mode, Some(RolloutMode::Running));
}

#[test]
fn operator_pause_requires_explicit_resume_and_stale_health_pauses_again() {
    let plan = plan();
    let mut simulator = CanaryRolloutSimulator::new(policy()).expect("simulator");
    simulator.apply(&register_command()).expect("register");
    simulator
        .apply(&observe_command(38, health(1, START)))
        .expect("health");
    simulator
        .apply(&RolloutCommand::Start {
            command_id: RolloutCommandId(bytes(39)),
            plan_id: plan.plan_id,
            recorded_at_ns: START,
        })
        .expect("start");
    simulator
        .apply(&RolloutCommand::OperatorPause {
            command_id: RolloutCommandId(bytes(62)),
            plan_id: plan.plan_id,
            operator_id: bytes(94),
            reason_digest: bytes(95),
            recorded_at_ns: START + 1,
        })
        .expect("pause");
    simulator
        .apply(&observe_command(63, health(2, START + 2)))
        .expect("health");
    assert_eq!(simulator.snapshot().mode, Some(RolloutMode::Paused));
    simulator
        .apply(&RolloutCommand::OperatorResume {
            command_id: RolloutCommandId(bytes(64)),
            plan_id: plan.plan_id,
            operator_id: bytes(96),
            recorded_at_ns: START + 3,
        })
        .expect("resume");
    let detail = simulator
        .apply(&RolloutCommand::Tick {
            command_id: RolloutCommandId(bytes(65)),
            plan_id: plan.plan_id,
            recorded_at_ns: START + 103,
        })
        .expect("tick")
        .detail;
    assert_eq!(detail, RolloutDetail::Paused(PauseReason::HealthStale));
    assert_eq!(simulator.snapshot().mode, Some(RolloutMode::Paused));
}

#[test]
fn every_severe_health_class_latches_rollback() {
    let cases = [
        (RollbackTrigger::CapitalFloorBreach, 0),
        (RollbackTrigger::ReconciliationTimeout, 1),
        (RollbackTrigger::UnknownStateTimeout, 2),
        (RollbackTrigger::SessionLossLimit, 3),
        (RollbackTrigger::ConsecutiveFaultLimit, 4),
    ];
    for (expected, profile) in cases {
        let mut frame = health(1, START);
        match profile {
            0 => frame.capital_floor_preserved = false,
            1 => {
                frame.reconciliation_healthy = false;
                frame.unreconciled_age_ns = 11;
            }
            2 => frame.unknown_state_age_ns = 11,
            3 => frame.session_loss_micros = 101,
            4 => frame.consecutive_faults = 2,
            _ => unreachable!(),
        }
        frame = frame.sealed();
        let mut simulator = CanaryRolloutSimulator::new(policy()).expect("simulator");
        simulator.apply(&register_command()).expect("register");
        simulator
            .apply(&observe_command(40 + profile, frame))
            .expect("health");
        assert_eq!(
            simulator.snapshot().mode,
            Some(RolloutMode::RollbackRequired)
        );
        assert_eq!(simulator.snapshot().rollback_trigger, Some(expected));
    }
}

#[test]
fn exact_duration_and_loss_limits_pass_but_stage_and_plan_excess_roll_back() {
    let mut boundary = health(1, START);
    boundary.unreconciled_age_ns = 10;
    boundary.unknown_state_age_ns = 10;
    boundary.session_loss_micros = 100;
    boundary.consecutive_faults = 1;
    boundary = boundary.sealed();
    let mut simulator = CanaryRolloutSimulator::new(policy()).expect("simulator");
    simulator.apply(&register_command()).expect("register");
    simulator
        .apply(&observe_command(45, boundary))
        .expect("boundary");
    assert_eq!(simulator.snapshot().mode, Some(RolloutMode::Registered));
    simulator
        .apply(&RolloutCommand::Start {
            command_id: RolloutCommandId(bytes(46)),
            plan_id: plan().plan_id,
            recorded_at_ns: START,
        })
        .expect("start");
    let detail = simulator
        .apply(&RolloutCommand::Tick {
            command_id: RolloutCommandId(bytes(47)),
            plan_id: plan().plan_id,
            recorded_at_ns: START + 301,
        })
        .expect("tick")
        .detail;
    assert_eq!(
        detail,
        RolloutDetail::RollbackLatched(RollbackTrigger::StageTimeout)
    );

    let mut plan_timeout = CanaryRolloutSimulator::new(policy()).expect("simulator");
    plan_timeout.apply(&register_command()).expect("register");
    let detail = plan_timeout
        .apply(&RolloutCommand::Tick {
            command_id: RolloutCommandId(bytes(48)),
            plan_id: plan().plan_id,
            recorded_at_ns: 1_901,
        })
        .expect("tick")
        .detail;
    assert_eq!(
        detail,
        RolloutDetail::RollbackLatched(RollbackTrigger::PlanTimeout)
    );
}

#[test]
fn restart_requires_post_restart_health_and_returns_paused_before_abort() {
    let plan = plan();
    let mut simulator = CanaryRolloutSimulator::new(policy()).expect("simulator");
    simulator.apply(&register_command()).expect("register");
    simulator
        .apply(&observe_command(50, health(1, START)))
        .expect("health");
    simulator
        .apply(&RolloutCommand::Start {
            command_id: RolloutCommandId(bytes(51)),
            plan_id: plan.plan_id,
            recorded_at_ns: START,
        })
        .expect("start");
    simulator
        .apply(&RolloutCommand::Restart {
            command_id: RolloutCommandId(bytes(52)),
            plan_id: plan.plan_id,
            restart_id: bytes(53),
            recorded_at_ns: START + 10,
        })
        .expect("restart");
    simulator
        .apply(&observe_command(54, health(2, START + 11)))
        .expect("health");
    simulator
        .apply(&RolloutCommand::Recover {
            command_id: RolloutCommandId(bytes(55)),
            plan_id: plan.plan_id,
            recovery_epoch: 1,
            evidence_digest: bytes(56),
            recorded_at_ns: START + 12,
        })
        .expect("recover");
    assert_eq!(simulator.snapshot().mode, Some(RolloutMode::Paused));
    simulator
        .apply(&RolloutCommand::OperatorResume {
            command_id: RolloutCommandId(bytes(57)),
            plan_id: plan.plan_id,
            operator_id: bytes(91),
            recorded_at_ns: START + 13,
        })
        .expect("resume");
    simulator
        .apply(&RolloutCommand::OperatorAbort {
            command_id: RolloutCommandId(bytes(58)),
            plan_id: plan.plan_id,
            operator_id: bytes(92),
            reason_digest: bytes(93),
            recorded_at_ns: START + 14,
        })
        .expect("abort");
    simulator
        .apply(&RolloutCommand::Finalize {
            command_id: RolloutCommandId(bytes(59)),
            plan_id: plan.plan_id,
            report_id: bytes(60),
            recorded_at_ns: START + 15,
        })
        .expect("finalize");
    let report = simulator.snapshot().last_report.expect("report");
    assert_eq!(report.status, RolloutReportStatus::OperatorAborted);
    assert_eq!(report.restart_count, 1);
    assert_eq!(report.recovery_epoch, 1);
    assert_eq!(report.abort_operator_id, Some(bytes(92)));
}

#[test]
fn authority_bearing_eligibility_substitution_halts_before_plan_installation() {
    let mut plan = plan();
    plan.eligibility.deployment_authority_granted = true;
    plan.eligibility = seal_record(plan.eligibility);
    plan = plan.sealed(&policy());
    let mut simulator = CanaryRolloutSimulator::new(policy()).expect("simulator");
    assert!(matches!(
        simulator.apply(&RolloutCommand::RegisterPlan {
            command_id: RolloutCommandId(bytes(61)),
            plan: Box::new(plan),
            recorded_at_ns: CREATED,
        }),
        Err(Error::Eligibility)
    ));
    assert!(simulator.snapshot().plan_id.is_none());
    assert!(simulator.is_halted());
}

#[test]
fn rollout_report_is_create_new_checksummed_and_digest_verified() {
    let simulator = run(&successful_commands());
    let report = simulator.snapshot().last_report.expect("report");
    let directory = tempdir().expect("dir");
    let path = directory.path().join("rollout.report");
    write_rollout_report_create_new(&path, &report).expect("write");
    assert_eq!(read_rollout_report(&path).expect("read"), report);
    assert!(write_rollout_report_create_new(&path, &report).is_err());
    let mut bytes = std::fs::read(&path).expect("bytes");
    let index = bytes.len() - 1;
    bytes[index] ^= 1;
    std::fs::write(&path, bytes).expect("corrupt");
    assert!(matches!(
        read_rollout_report(&path),
        Err(RolloutReportFileError::Checksum)
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
    let commands = successful_commands();
    let directory = tempdir().expect("dir");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 512 * 1024,
            max_segment_records: 3,
        },
    )
    .expect("writer");
    let recovery = RolloutRecovery {
        simulator: CanaryRolloutSimulator::new(policy()).expect("simulator"),
        last_sequence: None,
    };
    let mut durable = DurableRolloutSimulator::new(writer, recovery).expect("durable");
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let expected = durable.simulator().snapshot().digest;
    let checkpoint = RolloutCheckpoint {
        sequence: u64::try_from(commands.len() - 1).expect("sequence"),
        rollout_digest: expected,
    };
    let checkpoint_path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&checkpoint_path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.simulator.snapshot().digest, expected);

    let recovery = RolloutRecovery {
        simulator: CanaryRolloutSimulator::new(policy()).expect("simulator"),
        last_sequence: None,
    };
    let mut failing =
        DurableRolloutSimulator::new(FailingJournal::default(), recovery).expect("failing durable");
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(RolloutStorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(RolloutStorageError::Halted(_))
    ));
    assert_eq!(failing.simulator().snapshot().accepted_commands, 0);
}

#[test]
fn identical_rollout_commands_have_identical_complete_digests() {
    let commands = successful_commands();
    let first = run(&commands);
    let second = run(&commands);
    assert_eq!(first.snapshot().digest, second.snapshot().digest);
    assert_eq!(first.snapshot().last_report, second.snapshot().last_report);
}

proptest! {
    #[test]
    fn unreconciled_timeout_is_monotonic(age in 0_i64..100) {
        let mut simulator = CanaryRolloutSimulator::new(policy()).expect("simulator");
        simulator.apply(&register_command()).expect("register");
        let mut frame = health(1, START);
        frame.unreconciled_age_ns = age;
        let trigger = simulator.severe_trigger(&frame);
        prop_assert_eq!(
            trigger == Some(RollbackTrigger::ReconciliationTimeout),
            age > rollback().maximum_unreconciled_ns,
        );
    }
}
