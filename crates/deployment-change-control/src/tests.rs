use super::*;
use deployment_adapter_certification::{
    AdapterCertificationReport, CertificationReason, CertificationStatus,
};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use tempfile::tempdir;

const PLAN_ID: [u8; 32] = [1; 32];

fn id(value: u8) -> ChangeCommandId {
    ChangeCommandId([value; 32])
}

fn policy() -> ChangeControlPolicy {
    ChangeControlPolicy {
        maximum_windows: 4,
        maximum_steps: 8,
        maximum_certificate_age_ns: 500,
        maximum_plan_age_ns: 1_000,
        maximum_approval_age_ns: 1_000,
        maximum_permission_lifetime_ns: 100,
    }
}

fn certificate() -> AdapterCertificationReport {
    let mut report = AdapterCertificationReport {
        report_id: [2; 32],
        campaign_id: [3; 32],
        campaign_digest: [4; 32],
        contract_digest: [5; 32],
        completion_report_digest: [6; 32],
        rollback_report_digest: [7; 32],
        preflight_report_digest: [8; 32],
        rollback_package_digest: [9; 32],
        regions: vec!["eu-west".to_owned(), "us-east".to_owned()],
        finalized_at_ns: 3_800,
        status: CertificationStatus::Certified,
        reasons: Vec::new(),
        fixture_count: 20,
        privilege_test_count: 7,
        recovery_drill_count: 4,
        covered_recovery_regions: vec!["eu-west".to_owned(), "us-east".to_owned()],
        manual_operator_execution_required: true,
        credential_material_created: false,
        authentication_authority_granted: false,
        deployment_authority_granted: false,
        rollback_execution_authority_granted: false,
        traffic_authority_granted: false,
        cloud_control_authority_granted: false,
        live_trading_authority_granted: false,
        report_digest: [0; 32],
    };
    report.report_digest = adapter_report_digest_for_test(&report);
    report
}

fn adapter_report_digest_for_test(value: &AdapterCertificationReport) -> [u8; 32] {
    let mut clone = value.clone();
    clone.report_digest = [0; 32];
    digest_json(b"deployment-adapter-cert-report-v2", &clone)
}

fn step(index: u32, region: &str, action: ChangeAction) -> ChangeStep {
    ChangeStep {
        step_id: [u8::try_from(index + 20).expect("id"); 32],
        index,
        region: region.to_owned(),
        action,
        subject_digest: [u8::try_from(index + 30).expect("subject"); 32],
        step_digest: [0; 32],
    }
    .sealed()
}

fn plan() -> ChangePlan {
    ChangePlan {
        plan_id: PLAN_ID,
        created_at_ns: 3_900,
        expires_at_ns: 4_600,
        certificate: certificate(),
        windows: vec![MaintenanceWindow {
            window_id: [10; 32],
            starts_at_ns: 4_000,
            ends_at_ns: 4_500,
            window_digest: [0; 32],
        }
        .sealed()],
        steps: vec![
            step(0, "eu-west", ChangeAction::ApplyConfiguration),
            step(1, "us-east", ChangeAction::ShiftTraffic),
            step(2, "eu-west", ChangeAction::VerifyHealth),
        ],
        emergency_policy: EmergencyRollbackPolicy {
            rollback_package_digest: [9; 32],
            rollback_runbook_digest: [11; 32],
            triggers: EmergencyTrigger::ALL.to_vec(),
            maximum_rollback_permission_lifetime_ns: 100,
            policy_digest: [0; 32],
        }
        .sealed(),
        control_policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn register() -> ChangeCommand {
    ChangeCommand::Register {
        command_id: id(1),
        plan: Box::new(plan()),
        recorded_at_ns: 3_900,
    }
}

fn approval(command: u8, role: ApprovalRole, operator: u8) -> ChangeCommand {
    ChangeCommand::RecordApproval {
        command_id: id(command),
        approval: ChangeApproval {
            approval_id: [command + 40; 32],
            plan_id: PLAN_ID,
            plan_digest: plan().plan_digest,
            role,
            operator_id: [operator; 32],
            decision: ApprovalDecision::Approve,
            decided_at_ns: 3_910 + i64::from(command - 2),
            valid_until_ns: 4_500,
            approval_digest: [0; 32],
        }
        .sealed(),
        recorded_at_ns: 3_910 + i64::from(command - 2),
    }
}

fn issue(command: u8, permission: u8, at: i64) -> ChangeCommand {
    ChangeCommand::IssuePermission {
        command_id: id(command),
        plan_id: PLAN_ID,
        permission_id: [permission; 32],
        valid_until_ns: at + 50,
        recorded_at_ns: at,
    }
}

fn expected_permission(
    permission_id: u8,
    kind: PermissionKind,
    step_index: u32,
    at: i64,
) -> ManualPermission {
    let value = plan();
    let target = &value.steps[usize::try_from(step_index).expect("index")];
    let mut permission = ManualPermission {
        permission_id: [permission_id; 32],
        plan_id: PLAN_ID,
        plan_digest: value.plan_digest,
        certificate_digest: value.certificate.report_digest,
        kind,
        step_index,
        step_digest: target.step_digest,
        issued_at_ns: at,
        valid_until_ns: at + 50,
        manual_operator_execution_required: true,
        credential_material_created: false,
        authentication_authority_granted: false,
        deployment_authority_granted: false,
        rollback_execution_authority_granted: false,
        traffic_authority_granted: false,
        cloud_control_authority_granted: false,
        permission_digest: [0; 32],
    };
    permission.permission_digest = permission_digest(&permission);
    permission
}

fn consume(
    command: u8,
    permission_id: u8,
    kind: PermissionKind,
    step_index: u32,
    issued_at: i64,
    at: i64,
) -> ChangeCommand {
    let permission = expected_permission(permission_id, kind, step_index, issued_at);
    ChangeCommand::ConsumePermission {
        command_id: id(command),
        plan_id: PLAN_ID,
        permission_id: [permission_id; 32],
        permission_digest: permission.permission_digest,
        operator_handoff_digest: [60 + command; 32],
        recorded_at_ns: at,
    }
}

fn successful_commands() -> Vec<ChangeCommand> {
    vec![
        register(),
        approval(2, ApprovalRole::Release, 70),
        approval(3, ApprovalRole::Risk, 71),
        issue(4, 80, 4_000),
        consume(5, 80, PermissionKind::Change, 0, 4_000, 4_001),
        issue(6, 81, 4_002),
        consume(7, 81, PermissionKind::Change, 1, 4_002, 4_003),
        issue(8, 82, 4_004),
        consume(9, 82, PermissionKind::Change, 2, 4_004, 4_005),
        ChangeCommand::Finalize {
            command_id: id(10),
            plan_id: PLAN_ID,
            report_id: [90; 32],
            recorded_at_ns: 4_006,
        },
    ]
}

fn run(commands: &[ChangeCommand]) -> DeploymentChangeControl {
    let mut owner = DeploymentChangeControl::new(policy()).expect("owner");
    for command in commands {
        owner.apply(command).expect("valid command");
    }
    owner
}

fn approved_owner() -> DeploymentChangeControl {
    run(&[
        register(),
        approval(2, ApprovalRole::Release, 70),
        approval(3, ApprovalRole::Risk, 71),
    ])
}

#[test]
fn ordered_one_use_handoffs_complete_without_external_authority() {
    let owner = run(&successful_commands());
    let report = owner.snapshot().last_report.expect("report");
    assert_eq!(
        report.status,
        ChangeReportStatus::SimulatedHandoffsCompleted
    );
    assert_eq!(report.consumed_change_steps, vec![0, 1, 2]);
    assert!(report.rolled_back_steps.is_empty());
    assert!(report.manual_operator_execution_required);
    assert!(!report.credential_material_created);
    assert!(!report.authentication_authority_granted);
    assert!(!report.deployment_authority_granted);
    assert!(!report.rollback_execution_authority_granted);
    assert!(!report.traffic_authority_granted);
    assert!(!report.cloud_control_authority_granted);
    assert!(!report.live_trading_authority_granted);
    assert!(report.verify_digest());
}

#[test]
fn stale_noncertified_or_authority_bearing_certificate_halts_registration() {
    for case in 0_u8..3 {
        let mut value = plan();
        match case {
            0 => value.certificate.status = CertificationStatus::NotCertified,
            1 => value.certificate.finalized_at_ns = 3_000,
            _ => value.certificate.deployment_authority_granted = true,
        }
        if case == 0 {
            value.certificate.reasons = vec![CertificationReason::MissingRecoveryRegion(
                "eu-west".to_owned(),
            )];
        }
        value.certificate.report_digest = adapter_report_digest_for_test(&value.certificate);
        value = value.sealed(&policy());
        let mut owner = DeploymentChangeControl::new(policy()).expect("owner");
        assert!(matches!(
            owner.apply(&ChangeCommand::Register {
                command_id: id(20 + case),
                plan: Box::new(value),
                recorded_at_ns: 3_900,
            }),
            Err(Error::Plan)
        ));
    }
}

#[test]
fn dual_control_requires_distinct_current_operators() {
    let mut owner = run(&[register(), approval(2, ApprovalRole::Release, 70)]);
    assert!(matches!(
        owner.apply(&approval(3, ApprovalRole::Risk, 70)),
        Err(Error::Approval)
    ));

    let mut owner = approved_owner();
    assert!(matches!(
        owner.apply(&issue(4, 80, 3_999)),
        Err(Error::GateClosed)
    ));
}

#[test]
fn permission_expiry_and_single_use_fail_closed() {
    let mut owner = approved_owner();
    owner.apply(&issue(4, 80, 4_000)).expect("issue");
    assert!(matches!(
        owner.apply(&consume(5, 80, PermissionKind::Change, 0, 4_000, 4_051)),
        Err(Error::Permission)
    ));

    let mut owner = approved_owner();
    owner.apply(&issue(4, 80, 4_000)).expect("issue");
    owner
        .apply(&consume(5, 80, PermissionKind::Change, 0, 4_000, 4_001))
        .expect("consume");
    assert!(matches!(
        owner.apply(&consume(6, 80, PermissionKind::Change, 0, 4_000, 4_002)),
        Err(Error::Permission)
    ));
}

#[test]
fn pause_invalidates_outstanding_permission_and_resume_issues_a_new_one() {
    let mut owner = approved_owner();
    owner.apply(&issue(4, 80, 4_000)).expect("issue");
    owner
        .apply(&ChangeCommand::Pause {
            command_id: id(5),
            plan_id: PLAN_ID,
            operator_id: [70; 32],
            reason_digest: [91; 32],
            recorded_at_ns: 4_001,
        })
        .expect("pause");
    assert!(owner.snapshot().active_permission.is_none());
    owner
        .apply(&ChangeCommand::Resume {
            command_id: id(6),
            plan_id: PLAN_ID,
            operator_id: [71; 32],
            recorded_at_ns: 4_002,
        })
        .expect("resume");
    owner.apply(&issue(7, 81, 4_003)).expect("new permission");
    assert_eq!(
        owner
            .snapshot()
            .active_permission
            .expect("permission")
            .permission_id,
        [81; 32]
    );
}

#[test]
fn abort_before_handoff_is_safe_but_after_handoff_requires_rollback() {
    let mut before = approved_owner();
    let result = before
        .apply(&ChangeCommand::Abort {
            command_id: id(4),
            plan_id: PLAN_ID,
            operator_id: [70; 32],
            reason_digest: [92; 32],
            recorded_at_ns: 4_000,
        })
        .expect("abort");
    assert_eq!(result.detail, ChangeDetail::Aborted);

    let mut after = approved_owner();
    after.apply(&issue(4, 80, 4_000)).expect("issue");
    after
        .apply(&consume(5, 80, PermissionKind::Change, 0, 4_000, 4_001))
        .expect("consume");
    let result = after
        .apply(&ChangeCommand::Abort {
            command_id: id(6),
            plan_id: PLAN_ID,
            operator_id: [70; 32],
            reason_digest: [92; 32],
            recorded_at_ns: 4_002,
        })
        .expect("rollback required");
    assert_eq!(
        result.detail,
        ChangeDetail::RollbackRequired(EmergencyTrigger::OperatorAbort)
    );
}

#[test]
fn severe_signal_rolls_back_consumed_steps_in_reverse_order() {
    let mut owner = approved_owner();
    owner.apply(&issue(4, 80, 4_000)).expect("issue");
    owner
        .apply(&consume(5, 80, PermissionKind::Change, 0, 4_000, 4_001))
        .expect("consume");
    owner.apply(&issue(6, 81, 4_002)).expect("issue");
    owner
        .apply(&consume(7, 81, PermissionKind::Change, 1, 4_002, 4_003))
        .expect("consume");
    owner
        .apply(&ChangeCommand::SignalEmergency {
            command_id: id(8),
            plan_id: PLAN_ID,
            trigger: EmergencyTrigger::ReconciliationFailure,
            evidence_digest: [93; 32],
            recorded_at_ns: 4_004,
        })
        .expect("emergency");
    let first = owner.apply(&issue(9, 82, 4_005)).expect("rollback issue");
    let ChangeDetail::PermissionIssued(first) = first.detail else {
        panic!("permission expected");
    };
    assert_eq!(first.step_index, 1);
    owner
        .apply(&consume(10, 82, PermissionKind::Rollback, 1, 4_005, 4_006))
        .expect("rollback consume");
    let second = owner.apply(&issue(11, 83, 4_007)).expect("rollback issue");
    let ChangeDetail::PermissionIssued(second) = second.detail else {
        panic!("permission expected");
    };
    assert_eq!(second.step_index, 0);
    owner
        .apply(&consume(12, 83, PermissionKind::Rollback, 0, 4_007, 4_008))
        .expect("rollback consume");
    assert_eq!(owner.snapshot().mode, Some(ChangeMode::RolledBack));
    assert_eq!(owner.snapshot().rolled_back_steps, vec![1, 0]);
}

#[test]
fn report_file_is_create_new_and_corruption_detecting() {
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
        Err(ChangeControlReportFileError::Io(_))
    ));
    let mut bytes = std::fs::read(&path).expect("bytes");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, bytes).expect("corrupt");
    assert!(matches!(
        read_report(&path),
        Err(ChangeControlReportFileError::Checksum)
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
    let recovery = ChangeControlRecovery {
        change_control: DeploymentChangeControl::new(policy()).expect("owner"),
        last_sequence: None,
    };
    let mut durable = DurableChangeControl::new(writer, recovery).expect("durable");
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let expected = durable.change_control().snapshot().digest;
    let checkpoint = ChangeControlCheckpoint {
        sequence: u64::try_from(commands.len() - 1).expect("sequence"),
        change_control_digest: expected,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.change_control.snapshot().digest, expected);

    let recovery = ChangeControlRecovery {
        change_control: DeploymentChangeControl::new(policy()).expect("owner"),
        last_sequence: None,
    };
    let mut failing =
        DurableChangeControl::new(FailingJournal::default(), recovery).expect("owner");
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(ChangeControlStorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(ChangeControlStorageError::Halted(_))
    ));
    assert_eq!(failing.change_control().snapshot().accepted_commands, 0);
}

#[test]
fn identical_commands_produce_identical_complete_state() {
    let commands = successful_commands();
    assert_eq!(run(&commands).snapshot(), run(&commands).snapshot());
}

proptest! {
    #[test]
    fn permission_lifetime_above_policy_never_issues(extra in 1_i64..1_000) {
        let mut owner = approved_owner();
        let result = owner.apply(&ChangeCommand::IssuePermission {
            command_id: id(4),
            plan_id: PLAN_ID,
            permission_id: [80; 32],
            valid_until_ns: 4_000 + policy().maximum_permission_lifetime_ns + extra,
            recorded_at_ns: 4_000,
        });
        prop_assert!(matches!(result, Err(Error::Permission)));
    }
}
