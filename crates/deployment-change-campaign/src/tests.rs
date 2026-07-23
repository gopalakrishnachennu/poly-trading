use super::*;
use deployment_adapter_certification::{AdapterCertificationReport, CertificationStatus};
use deployment_change_control::{
    ApprovalDecision, ChangeAction, ChangeApproval, ChangeCommandId, ChangePlan, ChangeStep,
    EmergencyRollbackPolicy, MaintenanceWindow, ManualPermission,
};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(value: u8) -> [u8; 32] {
    [value; 32]
}

fn campaign_policy() -> ChangeCampaignPolicy {
    ChangeCampaignPolicy {
        minimum_independent_plans: 2,
        maximum_cases: 8,
        maximum_commands_per_case: 32,
        maximum_campaign_age_ns: 50_000,
    }
}

fn change_policy() -> ChangeControlPolicy {
    ChangeControlPolicy {
        maximum_windows: 4,
        maximum_steps: 8,
        maximum_certificate_age_ns: 500,
        maximum_plan_age_ns: 1_000,
        maximum_approval_age_ns: 900,
        maximum_permission_lifetime_ns: 100,
    }
}

fn certificate(base: i64, seed: u8) -> AdapterCertificationReport {
    let mut report = AdapterCertificationReport {
        report_id: id(seed),
        campaign_id: id(seed + 1),
        campaign_digest: id(seed + 2),
        contract_digest: id(seed + 3),
        completion_report_digest: id(seed + 4),
        rollback_report_digest: id(seed + 5),
        preflight_report_digest: id(seed + 6),
        rollback_package_digest: id(seed + 7),
        regions: vec!["eu-west".to_owned(), "us-east".to_owned()],
        finalized_at_ns: base - 100,
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
    report.report_digest = adapter_report_digest(&report);
    report
}

fn adapter_report_digest(value: &AdapterCertificationReport) -> [u8; 32] {
    let mut clone = value.clone();
    clone.report_digest = [0; 32];
    digest_json(b"deployment-adapter-cert-report-v2", &clone)
}

fn make_step(index: u32, seed: u8) -> ChangeStep {
    ChangeStep {
        step_id: id(seed + u8::try_from(index).expect("step")),
        index,
        region: if index % 2 == 0 {
            "eu-west".to_owned()
        } else {
            "us-east".to_owned()
        },
        action: match index {
            0 => ChangeAction::ApplyConfiguration,
            1 => ChangeAction::ShiftTraffic,
            _ => ChangeAction::VerifyHealth,
        },
        subject_digest: id(seed + 10 + u8::try_from(index).expect("subject")),
        step_digest: [0; 32],
    }
    .sealed()
}

fn make_plan(base: i64, seed: u8, step_count: u32) -> ChangePlan {
    let cert = certificate(base, seed + 30);
    ChangePlan {
        plan_id: id(seed),
        created_at_ns: base,
        expires_at_ns: base + 900,
        certificate: cert.clone(),
        windows: vec![
            MaintenanceWindow {
                window_id: id(seed + 1),
                starts_at_ns: base + 100,
                ends_at_ns: base + 200,
                window_digest: [0; 32],
            }
            .sealed(),
            MaintenanceWindow {
                window_id: id(seed + 2),
                starts_at_ns: base + 300,
                ends_at_ns: base + 400,
                window_digest: [0; 32],
            }
            .sealed(),
        ],
        steps: (0..step_count)
            .map(|index| make_step(index, seed + 3))
            .collect(),
        emergency_policy: EmergencyRollbackPolicy {
            rollback_package_digest: cert.rollback_package_digest,
            rollback_runbook_digest: id(seed + 20),
            triggers: EmergencyTrigger::ALL.to_vec(),
            maximum_rollback_permission_lifetime_ns: 100,
            policy_digest: [0; 32],
        }
        .sealed(),
        control_policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&change_policy())
}

fn command_id(seed: u8, index: u8) -> ChangeCommandId {
    ChangeCommandId(id(seed.wrapping_add(index)))
}

fn register(plan: &ChangePlan, seed: u8) -> ChangeCommand {
    ChangeCommand::Register {
        command_id: command_id(seed, 1),
        plan: Box::new(plan.clone()),
        recorded_at_ns: plan.created_at_ns,
    }
}

fn approval(
    plan: &ChangePlan,
    seed: u8,
    index: u8,
    role: ApprovalRole,
    operator: u8,
    at: i64,
    valid_until: i64,
) -> ChangeCommand {
    ChangeCommand::RecordApproval {
        command_id: command_id(seed, index),
        approval: ChangeApproval {
            approval_id: id(seed.wrapping_add(index).wrapping_add(80)),
            plan_id: plan.plan_id,
            plan_digest: plan.plan_digest,
            role,
            operator_id: id(operator),
            decision: ApprovalDecision::Approve,
            decided_at_ns: at,
            valid_until_ns: valid_until,
            approval_digest: [0; 32],
        }
        .sealed(),
        recorded_at_ns: at,
    }
}

fn push(
    owner: &mut DeploymentChangeControl,
    commands: &mut Vec<ChangeCommand>,
    command: ChangeCommand,
) {
    owner.apply(&command).expect("fixture command");
    commands.push(command);
}

fn issue(
    owner: &mut DeploymentChangeControl,
    commands: &mut Vec<ChangeCommand>,
    plan: &ChangePlan,
    seed: u8,
    index: u8,
    permission_id: u8,
    at: i64,
) -> ManualPermission {
    let command = ChangeCommand::IssuePermission {
        command_id: command_id(seed, index),
        plan_id: plan.plan_id,
        permission_id: id(permission_id),
        valid_until_ns: at + 50,
        recorded_at_ns: at,
    };
    let outcome = owner.apply(&command).expect("issue");
    commands.push(command);
    let ChangeDetail::PermissionIssued(permission) = outcome.detail else {
        panic!("permission outcome")
    };
    *permission
}

fn consume(
    owner: &mut DeploymentChangeControl,
    commands: &mut Vec<ChangeCommand>,
    plan: &ChangePlan,
    seed: u8,
    index: u8,
    permission: &ManualPermission,
    at: i64,
) {
    push(
        owner,
        commands,
        ChangeCommand::ConsumePermission {
            command_id: command_id(seed, index),
            plan_id: plan.plan_id,
            permission_id: permission.permission_id,
            permission_digest: permission.permission_digest,
            operator_handoff_digest: id(seed.wrapping_add(index).wrapping_add(100)),
            recorded_at_ns: at,
        },
    );
}

fn base_commands(
    plan: &ChangePlan,
    seed: u8,
    approval_until: i64,
) -> (DeploymentChangeControl, Vec<ChangeCommand>) {
    let mut owner = DeploymentChangeControl::new(change_policy()).expect("owner");
    let mut commands = Vec::new();
    push(&mut owner, &mut commands, register(plan, seed));
    push(
        &mut owner,
        &mut commands,
        approval(
            plan,
            seed,
            2,
            ApprovalRole::Release,
            seed + 70,
            plan.created_at_ns + 10,
            approval_until,
        ),
    );
    push(
        &mut owner,
        &mut commands,
        approval(
            plan,
            seed,
            3,
            ApprovalRole::Risk,
            seed + 71,
            plan.created_at_ns + 11,
            approval_until,
        ),
    );
    (owner, commands)
}

#[allow(clippy::too_many_lines)]
fn completed_case(base: i64, seed: u8) -> ChangeCampaignCase {
    let plan = make_plan(base, seed, 3);
    let (mut owner, mut commands) = base_commands(&plan, seed, base + 850);
    let permission = issue(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        4,
        seed + 50,
        base + 100,
    );
    consume(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        5,
        &permission,
        base + 101,
    );
    let _invalidated = issue(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        6,
        seed + 51,
        base + 120,
    );
    push(
        &mut owner,
        &mut commands,
        ChangeCommand::Pause {
            command_id: command_id(seed, 7),
            plan_id: plan.plan_id,
            operator_id: id(seed + 72),
            reason_digest: id(seed + 73),
            recorded_at_ns: base + 121,
        },
    );
    push(
        &mut owner,
        &mut commands,
        ChangeCommand::Resume {
            command_id: command_id(seed, 8),
            plan_id: plan.plan_id,
            operator_id: id(seed + 72),
            recorded_at_ns: base + 122,
        },
    );
    let permission = issue(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        9,
        seed + 52,
        base + 123,
    );
    consume(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        10,
        &permission,
        base + 124,
    );
    let permission = issue(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        11,
        seed + 53,
        base + 300,
    );
    consume(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        12,
        &permission,
        base + 301,
    );
    push(
        &mut owner,
        &mut commands,
        ChangeCommand::Finalize {
            command_id: command_id(seed, 13),
            plan_id: plan.plan_id,
            report_id: id(seed + 60),
            recorded_at_ns: base + 302,
        },
    );
    ChangeCampaignCase {
        case_id: id(seed + 61),
        change_policy: change_policy(),
        commands,
        expected_result: ExpectedCaseResult::Completed,
        restart_after_commands: Some(5),
        case_digest: [0; 32],
    }
    .sealed()
}

fn safe_abort_case(base: i64, seed: u8) -> ChangeCampaignCase {
    let plan = make_plan(base, seed, 1);
    let (mut owner, mut commands) = base_commands(&plan, seed, base + 850);
    push(
        &mut owner,
        &mut commands,
        ChangeCommand::Abort {
            command_id: command_id(seed, 4),
            plan_id: plan.plan_id,
            operator_id: id(seed + 72),
            reason_digest: id(seed + 73),
            recorded_at_ns: base + 20,
        },
    );
    push(
        &mut owner,
        &mut commands,
        ChangeCommand::Finalize {
            command_id: command_id(seed, 5),
            plan_id: plan.plan_id,
            report_id: id(seed + 60),
            recorded_at_ns: base + 21,
        },
    );
    ChangeCampaignCase {
        case_id: id(seed + 61),
        change_policy: change_policy(),
        commands,
        expected_result: ExpectedCaseResult::SafeAbort,
        restart_after_commands: None,
        case_digest: [0; 32],
    }
    .sealed()
}

fn rollback_case(base: i64, seed: u8) -> ChangeCampaignCase {
    let plan = make_plan(base, seed, 2);
    let (mut owner, mut commands) = base_commands(&plan, seed, base + 850);
    let permission = issue(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        4,
        seed + 50,
        base + 100,
    );
    consume(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        5,
        &permission,
        base + 101,
    );
    push(
        &mut owner,
        &mut commands,
        ChangeCommand::SignalEmergency {
            command_id: command_id(seed, 6),
            plan_id: plan.plan_id,
            trigger: EmergencyTrigger::ReconciliationFailure,
            evidence_digest: id(seed + 74),
            recorded_at_ns: base + 110,
        },
    );
    let permission = issue(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        7,
        seed + 51,
        base + 111,
    );
    consume(
        &mut owner,
        &mut commands,
        &plan,
        seed,
        8,
        &permission,
        base + 112,
    );
    push(
        &mut owner,
        &mut commands,
        ChangeCommand::Finalize {
            command_id: command_id(seed, 9),
            plan_id: plan.plan_id,
            report_id: id(seed + 60),
            recorded_at_ns: base + 113,
        },
    );
    ChangeCampaignCase {
        case_id: id(seed + 61),
        change_policy: change_policy(),
        commands,
        expected_result: ExpectedCaseResult::EmergencyRolledBack,
        restart_after_commands: None,
        case_digest: [0; 32],
    }
    .sealed()
}

fn expiry_case(base: i64, seed: u8) -> ChangeCampaignCase {
    let plan = make_plan(base, seed, 1);
    let (_owner, mut commands) = base_commands(&plan, seed, base + 150);
    commands.push(ChangeCommand::IssuePermission {
        command_id: command_id(seed, 4),
        plan_id: plan.plan_id,
        permission_id: id(seed + 50),
        valid_until_ns: base + 350,
        recorded_at_ns: base + 300,
    });
    ChangeCampaignCase {
        case_id: id(seed + 61),
        change_policy: change_policy(),
        commands,
        expected_result: ExpectedCaseResult::ApprovalExpiryDenied,
        restart_after_commands: None,
        case_digest: [0; 32],
    }
    .sealed()
}

fn manifest() -> ChangeCampaignManifest {
    ChangeCampaignManifest {
        campaign_id: id(250),
        created_at_ns: 9_000,
        expires_at_ns: 50_000,
        cases: vec![
            completed_case(10_000, 1),
            safe_abort_case(20_000, 40),
            rollback_case(30_000, 80),
            expiry_case(40_000, 120),
        ],
        required_scenarios: RequiredScenario::ALL.to_vec(),
        expected_case_schedule_digest: [0; 32],
        campaign_policy_digest: [0; 32],
        manifest_digest: [0; 32],
    }
    .sealed(&campaign_policy())
}

fn successful_commands() -> Vec<CampaignCommand> {
    let manifest = manifest();
    let mut commands = vec![CampaignCommand::Register {
        command_id: CampaignCommandId(id(1)),
        recorded_at_ns: manifest.created_at_ns,
        manifest: Box::new(manifest.clone()),
    }];
    for (index, case) in manifest.cases.iter().enumerate() {
        commands.push(CampaignCommand::RunNextCase {
            command_id: CampaignCommandId(id(u8::try_from(index + 2).expect("command"))),
            campaign_id: manifest.campaign_id,
            case_id: case.case_id,
            recorded_at_ns: case.commands.last().expect("last").recorded_at_ns(),
        });
    }
    commands.push(CampaignCommand::Finalize {
        command_id: CampaignCommandId(id(10)),
        campaign_id: manifest.campaign_id,
        evidence_id: id(251),
        evaluated_at_ns: 50_000,
        recorded_at_ns: 50_000,
    });
    commands
}

fn run(commands: &[CampaignCommand]) -> DeploymentChangeCampaign {
    let mut owner = DeploymentChangeCampaign::new(campaign_policy()).expect("owner");
    for command in commands {
        owner.apply(command).expect("campaign command");
    }
    owner
}

#[test]
fn full_campaign_derives_every_scenario_without_external_authority() {
    let owner = run(&successful_commands());
    let snapshot = owner.snapshot();
    let evidence = snapshot.last_evidence.expect("evidence");
    assert_eq!(
        evidence.status,
        CampaignEvidenceStatus::OperatorReviewEligible
    );
    assert!(evidence.reasons.is_empty());
    assert_eq!(evidence.covered_scenarios, RequiredScenario::ALL);
    assert_eq!(evidence.completed_case_count, 4);
    assert_eq!(evidence.independent_plan_count, 4);
    assert_eq!(evidence.restart_reconstruction_count, 1);
    assert!(evidence.operator_decision_required);
    assert!(!evidence.credential_material_created);
    assert!(!evidence.authentication_authority_granted);
    assert!(!evidence.deployment_authority_granted);
    assert!(!evidence.rollback_execution_authority_granted);
    assert!(!evidence.traffic_authority_granted);
    assert!(!evidence.cloud_control_authority_granted);
    assert!(!evidence.live_trading_authority_granted);
}

#[test]
fn expiry_coverage_requires_authentic_child_approval_halt() {
    let case = expiry_case(40_000, 120);
    let execution = execute_case(&case).expect("execute");
    assert!(execution.result.child_halted_as_expected);
    assert!(execution
        .result
        .covered_scenarios
        .contains(&RequiredScenario::ApprovalExpiryDenial));

    let mut invalid = case;
    let last = invalid.commands.last_mut().expect("last");
    let ChangeCommand::IssuePermission { recorded_at_ns, .. } = last else {
        panic!("issue")
    };
    *recorded_at_ns = 40_100;
    invalid = invalid.sealed();
    assert!(execute_case(&invalid).is_err());
}

#[test]
fn out_of_order_case_and_plan_substitution_halt() {
    let manifest = manifest();
    let mut owner = DeploymentChangeCampaign::new(campaign_policy()).expect("owner");
    owner
        .apply(&CampaignCommand::Register {
            command_id: CampaignCommandId(id(1)),
            recorded_at_ns: manifest.created_at_ns,
            manifest: Box::new(manifest.clone()),
        })
        .expect("register");
    let error = owner.apply(&CampaignCommand::RunNextCase {
        command_id: CampaignCommandId(id(2)),
        campaign_id: manifest.campaign_id,
        case_id: manifest.cases[1].case_id,
        recorded_at_ns: 20_021,
    });
    assert!(matches!(error, Err(Error::Case)));
    assert!(owner.is_halted());

    let mut duplicate = manifest;
    let plan = duplicate.cases[0].commands[0].clone();
    duplicate.cases[1].commands[0] = plan;
    duplicate.cases[1] = duplicate.cases[1].clone().sealed();
    duplicate = duplicate.sealed(&campaign_policy());
    let mut owner = DeploymentChangeCampaign::new(campaign_policy()).expect("owner");
    assert!(owner
        .apply(&CampaignCommand::Register {
            command_id: CampaignCommandId(id(3)),
            recorded_at_ns: duplicate.created_at_ns,
            manifest: Box::new(duplicate),
        })
        .is_err());
}

#[test]
fn premature_finalization_is_attributable_and_noneligible() {
    let manifest = manifest();
    let commands = vec![
        CampaignCommand::Register {
            command_id: CampaignCommandId(id(1)),
            recorded_at_ns: manifest.created_at_ns,
            manifest: Box::new(manifest.clone()),
        },
        CampaignCommand::Finalize {
            command_id: CampaignCommandId(id(2)),
            campaign_id: manifest.campaign_id,
            evidence_id: id(249),
            evaluated_at_ns: 9_100,
            recorded_at_ns: 9_100,
        },
    ];
    let evidence = run(&commands).snapshot().last_evidence.expect("evidence");
    assert_eq!(evidence.status, CampaignEvidenceStatus::NotEligible);
    assert!(evidence
        .reasons
        .contains(&CampaignEvidenceReason::CasesIncomplete));
    assert!(evidence
        .reasons
        .contains(&CampaignEvidenceReason::IndependentPlanFloor));
}

#[test]
fn evidence_file_is_create_new_and_corruption_detecting() {
    let evidence = run(&successful_commands())
        .snapshot()
        .last_evidence
        .expect("evidence");
    let directory = tempdir().expect("directory");
    let path = directory.path().join("campaign.evidence");
    write_evidence_create_new(&path, &evidence).expect("write");
    assert_eq!(read_evidence(&path).expect("read"), evidence);
    assert!(matches!(
        write_evidence_create_new(&path, &evidence),
        Err(ChangeCampaignEvidenceFileError::Io(_))
    ));
    let mut bytes = std::fs::read(&path).expect("bytes");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, bytes).expect("corrupt");
    assert!(matches!(
        read_evidence(&path),
        Err(ChangeCampaignEvidenceFileError::Checksum)
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
            max_segment_bytes: 4 * 1024 * 1024,
            max_segment_records: 2,
        },
    )
    .expect("writer");
    let recovery = CampaignRecovery {
        campaign: DeploymentChangeCampaign::new(campaign_policy()).expect("owner"),
        last_sequence: None,
    };
    let mut durable = DurableChangeCampaign::new(writer, recovery).expect("durable");
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let expected = durable.campaign().snapshot().digest;
    let checkpoint = CampaignCheckpoint {
        sequence: u64::try_from(commands.len() - 1).expect("sequence"),
        campaign_digest: expected,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered =
        recover_segmented(&segments, campaign_policy(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.campaign.snapshot().digest, expected);

    let recovery = CampaignRecovery {
        campaign: DeploymentChangeCampaign::new(campaign_policy()).expect("owner"),
        last_sequence: None,
    };
    let mut failing =
        DurableChangeCampaign::new(FailingJournal::default(), recovery).expect("owner");
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(CampaignStorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(CampaignStorageError::Halted(_))
    ));
    assert_eq!(failing.campaign().snapshot().accepted_commands, 0);
}

#[test]
fn identical_commands_produce_identical_complete_state() {
    let commands = successful_commands();
    assert_eq!(run(&commands).snapshot(), run(&commands).snapshot());
}

proptest! {
    #[test]
    fn insufficient_case_capacity_never_constructs(extra in 1_usize..32) {
        let policy = ChangeCampaignPolicy {
            minimum_independent_plans: 2 + extra,
            maximum_cases: 1,
            maximum_commands_per_case: 32,
            maximum_campaign_age_ns: 50_000,
        };
        prop_assert!(matches!(DeploymentChangeCampaign::new(policy), Err(Error::Config)));
    }
}
