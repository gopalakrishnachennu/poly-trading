use super::*;
use deployment_orchestration_simulator::{OrchestrationReportStatus, RollbackTrigger};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use tempfile::tempdir;

const CAMPAIGN_ID: [u8; 32] = [1; 32];
const CONTRACT_ID: [u8; 32] = [2; 32];
const PREFLIGHT_DIGEST: [u8; 32] = [3; 32];
const ROLLBACK_PACKAGE: [u8; 32] = [4; 32];

fn id(value: u8) -> CertificationCommandId {
    CertificationCommandId([value; 32])
}

fn policy() -> CertificationPolicy {
    CertificationPolicy {
        maximum_regions: 4,
        maximum_report_age_ns: 500,
        maximum_campaign_age_ns: 2_000,
        maximum_fixture_age_ns: 1_500,
        maximum_recovery_duration_ns: 100,
    }
}

fn orchestration_report(report_id: u8, status: OrchestrationReportStatus) -> OrchestrationReport {
    let rolled_back = if status == OrchestrationReportStatus::SimulatedRolledBack {
        vec!["us-east".to_owned(), "eu-west".to_owned()]
    } else {
        Vec::new()
    };
    let mut report = OrchestrationReport {
        report_id: [report_id; 32],
        plan_id: [report_id + 10; 32],
        plan_digest: [report_id + 20; 32],
        preflight_report_digest: PREFLIGHT_DIGEST,
        rollback_package_digest: ROLLBACK_PACKAGE,
        finalized_at_ns: if status == OrchestrationReportStatus::SimulatedCompleted {
            2_900
        } else {
            2_950
        },
        status,
        completed_wave_count: 2,
        activated_regions: vec!["eu-west".to_owned(), "us-east".to_owned()],
        rolled_back_regions: rolled_back,
        rollback_trigger: if status == OrchestrationReportStatus::SimulatedRolledBack {
            Some(RollbackTrigger::ReconciliationFailure)
        } else {
            None
        },
        pause_count: 1,
        restart_count: 1,
        recovery_epoch: 1,
        manual_operator_execution_required: true,
        credential_material_created: false,
        deployment_authority_granted: false,
        rollback_execution_authority_granted: false,
        cloud_control_authority_granted: false,
        live_trading_authority_granted: false,
        report_digest: [0; 32],
    };
    report.report_digest = orchestration_report_digest_for_test(&report);
    report
}

fn orchestration_report_digest_for_test(value: &OrchestrationReport) -> [u8; 32] {
    let mut clone = value.clone();
    clone.report_digest = [0; 32];
    digest_json(b"deployment-orchestration-report-v1", &clone)
}

fn privilege_policy() -> AdapterPrivilegePolicy {
    AdapterPrivilegePolicy {
        allowed_operations: vec![
            AdapterOperation::ReadState,
            AdapterOperation::ServerSideDryRun,
            AdapterOperation::PlanApply,
            AdapterOperation::PlanTrafficShift,
            AdapterOperation::PlanRollback,
        ],
        allowed_resource_digest: [5; 32],
        credential_material_allowed: false,
        wildcard_resources_allowed: false,
        secret_read_allowed: false,
        cluster_admin_allowed: false,
        arbitrary_exec_allowed: false,
        privilege_escalation_allowed: false,
        cross_region_mutation_allowed: false,
        policy_digest: [0; 32],
    }
    .sealed()
}

fn contract() -> DeploymentAdapterContract {
    DeploymentAdapterContract {
        contract_id: CONTRACT_ID,
        interface_schema_digest: [6; 32],
        deployment_manifest_digest: [7; 32],
        rollback_manifest_digest: [8; 32],
        recovery_runbook_digest: [9; 32],
        regions: vec!["eu-west".to_owned(), "us-east".to_owned()],
        privilege_policy: privilege_policy(),
        contract_digest: [0; 32],
    }
    .sealed()
}

fn campaign() -> CertificationCampaign {
    CertificationCampaign {
        campaign_id: CAMPAIGN_ID,
        created_at_ns: 3_000,
        expires_at_ns: 5_000,
        completion_report: orchestration_report(10, OrchestrationReportStatus::SimulatedCompleted),
        rollback_report: orchestration_report(11, OrchestrationReportStatus::SimulatedRolledBack),
        adapter_contract: contract(),
        policy_digest: [0; 32],
        campaign_digest: [0; 32],
    }
    .sealed(&policy())
}

fn register() -> CertificationCommand {
    CertificationCommand::Register {
        command_id: id(1),
        campaign: Box::new(campaign()),
        recorded_at_ns: 3_000,
    }
}

fn fixture(
    command: u8,
    fixture_id: u8,
    region: &str,
    class: FixtureClass,
    at: i64,
) -> CertificationCommand {
    CertificationCommand::RecordFixture {
        command_id: id(command),
        fixture: RecordedAdapterFixture {
            fixture_id: [fixture_id; 32],
            campaign_id: CAMPAIGN_ID,
            contract_digest: contract().contract_digest,
            region: region.to_owned(),
            sequence: class.index(),
            class,
            disposition: expected_disposition(class),
            observed_at_ns: at,
            source_digest: [12; 32],
            request_digest: [13; 32],
            response_digest: [14; 32],
            mutation_performed: false,
            credential_loaded: false,
            fixture_digest: [0; 32],
        }
        .sealed(),
        recorded_at_ns: at,
    }
}

fn privilege(command: u8, class: PrivilegeTestClass, at: i64) -> CertificationCommand {
    CertificationCommand::RecordPrivilegeTest {
        command_id: id(command),
        evidence: PrivilegeTestEvidence {
            evidence_id: [command + 60; 32],
            campaign_id: CAMPAIGN_ID,
            policy_digest: privilege_policy().policy_digest,
            sequence: class.index(),
            class,
            result: if class == PrivilegeTestClass::BaselinePolicyData {
                PrivilegeTestResult::PolicyDataAccepted
            } else {
                PrivilegeTestResult::Denied
            },
            observed_at_ns: at,
            source_digest: [15; 32],
            credential_loaded: false,
            signature_created: false,
            executable_request_created: false,
            evidence_digest: [0; 32],
        }
        .sealed(),
        recorded_at_ns: at,
    }
}

fn recovery(
    command: u8,
    scenario: RecoveryScenario,
    failed: &str,
    recovered: &str,
    at: i64,
) -> CertificationCommand {
    CertificationCommand::RecordRecoveryDrill {
        command_id: id(command),
        evidence: RecoveryDrillEvidence {
            drill_id: [command + 80; 32],
            campaign_id: CAMPAIGN_ID,
            contract_digest: contract().contract_digest,
            rollback_package_digest: ROLLBACK_PACKAGE,
            sequence: scenario.index(),
            scenario,
            failed_region: failed.to_owned(),
            recovery_region: recovered.to_owned(),
            started_at_ns: at - 20,
            recovered_at_ns: at,
            journal_replayed: true,
            checkpoint_verified: true,
            reconciliation_restored: true,
            rollback_available: true,
            failover_observed: true,
            manual_promotion_required: true,
            traffic_shift_performed: false,
            credential_loaded: false,
            source_digest: [16; 32],
            evidence_digest: [0; 32],
        }
        .sealed(),
        recorded_at_ns: at,
    }
}

fn complete_commands() -> Vec<CertificationCommand> {
    let mut commands = vec![register()];
    let mut command = 2_u8;
    let mut fixture_id = 20_u8;
    let mut at = 3_010_i64;
    for region in ["eu-west", "us-east"] {
        for class in FixtureClass::ALL {
            commands.push(fixture(command, fixture_id, region, class, at));
            command += 1;
            fixture_id += 1;
            at += 1;
        }
    }
    for class in PrivilegeTestClass::ALL {
        commands.push(privilege(command, class, at));
        command += 1;
        at += 1;
    }
    for (scenario, failed, recovered) in [
        (RecoveryScenario::RegionUnavailable, "eu-west", "us-east"),
        (
            RecoveryScenario::ControlPlanePartition,
            "us-east",
            "eu-west",
        ),
        (RecoveryScenario::DurableStateLoss, "eu-west", "us-east"),
        (RecoveryScenario::ArtifactUnavailable, "us-east", "eu-west"),
    ] {
        commands.push(recovery(command, scenario, failed, recovered, at));
        command += 1;
        at += 1;
    }
    commands.push(CertificationCommand::Finalize {
        command_id: id(command),
        campaign_id: CAMPAIGN_ID,
        report_id: [50; 32],
        recorded_at_ns: at,
    });
    commands
}

fn run(commands: &[CertificationCommand]) -> DeploymentAdapterCertification {
    let mut owner = DeploymentAdapterCertification::new(policy()).expect("owner");
    for command in commands {
        owner.apply(command).expect("valid command");
    }
    owner
}

#[test]
fn complete_recorded_campaign_certifies_without_external_authority() {
    let owner = run(&complete_commands());
    let snapshot = owner.snapshot();
    assert_eq!(snapshot.fixture_count, 20);
    assert_eq!(snapshot.privilege_test_count, 7);
    assert_eq!(snapshot.recovery_drill_count, 4);
    let report = snapshot.last_report.expect("report");
    assert_eq!(report.status, CertificationStatus::Certified);
    assert!(report.reasons.is_empty());
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
fn incomplete_campaign_is_attributable_and_not_certified() {
    let owner = run(&[
        register(),
        CertificationCommand::Finalize {
            command_id: id(2),
            campaign_id: CAMPAIGN_ID,
            report_id: [51; 32],
            recorded_at_ns: 3_010,
        },
    ]);
    let report = owner.snapshot().last_report.expect("report");
    assert_eq!(report.status, CertificationStatus::NotCertified);
    assert_eq!(report.reasons.len(), 33);
}

#[test]
fn evidence_is_rechecked_for_freshness_at_finalization() {
    let owner = run(&[
        register(),
        fixture(2, 20, "eu-west", FixtureClass::DiscoverState, 3_010),
        CertificationCommand::Finalize {
            command_id: id(3),
            campaign_id: CAMPAIGN_ID,
            report_id: [52; 32],
            recorded_at_ns: 4_600,
        },
    ]);
    let report = owner.snapshot().last_report.expect("report");
    assert_eq!(report.status, CertificationStatus::NotCertified);
    assert!(report.reasons.contains(&CertificationReason::StaleFixture {
        region: "eu-west".to_owned(),
        class: FixtureClass::DiscoverState,
    }));
}

#[test]
fn authority_bearing_or_substituted_orchestration_report_halts_registration() {
    for substitute in [false, true] {
        let mut value = campaign();
        if substitute {
            value.rollback_report.rollback_package_digest = [99; 32];
        } else {
            value.completion_report.deployment_authority_granted = true;
        }
        value.completion_report.report_digest =
            orchestration_report_digest_for_test(&value.completion_report);
        value.rollback_report.report_digest =
            orchestration_report_digest_for_test(&value.rollback_report);
        value = value.sealed(&policy());
        let mut owner = DeploymentAdapterCertification::new(policy()).expect("owner");
        assert!(matches!(
            owner.apply(&CertificationCommand::Register {
                command_id: id(if substitute { 70 } else { 71 }),
                campaign: Box::new(value),
                recorded_at_ns: 3_000,
            }),
            Err(Error::Campaign)
        ));
        assert!(owner.is_halted());
    }
}

#[test]
fn fixture_sequence_gap_and_mutation_claim_halt() {
    let mut owner = run(&[register()]);
    assert!(matches!(
        owner.apply(&fixture(
            2,
            20,
            "eu-west",
            FixtureClass::ServerSideDryRun,
            3_010
        )),
        Err(Error::Fixture)
    ));

    let mut owner = run(&[register()]);
    let CertificationCommand::RecordFixture {
        fixture: mut unsafe_fixture,
        ..
    } = fixture(2, 20, "eu-west", FixtureClass::DiscoverState, 3_010)
    else {
        unreachable!();
    };
    unsafe_fixture.mutation_performed = true;
    unsafe_fixture = unsafe_fixture.sealed();
    assert!(matches!(
        owner.apply(&CertificationCommand::RecordFixture {
            command_id: id(2),
            fixture: unsafe_fixture,
            recorded_at_ns: 3_010,
        }),
        Err(Error::Fixture)
    ));
}

#[test]
fn privilege_escalation_or_executable_request_halts() {
    let mut owner = run(&[register()]);
    let CertificationCommand::RecordPrivilegeTest { mut evidence, .. } =
        privilege(2, PrivilegeTestClass::BaselinePolicyData, 3_010)
    else {
        unreachable!();
    };
    evidence.executable_request_created = true;
    evidence = evidence.sealed();
    assert!(matches!(
        owner.apply(&CertificationCommand::RecordPrivilegeTest {
            command_id: id(2),
            evidence,
            recorded_at_ns: 3_010,
        }),
        Err(Error::Privilege)
    ));
}

#[test]
fn incomplete_or_overlong_recovery_halts() {
    let mut owner = run(&[register()]);
    let CertificationCommand::RecordRecoveryDrill { mut evidence, .. } = recovery(
        2,
        RecoveryScenario::RegionUnavailable,
        "eu-west",
        "us-east",
        3_200,
    ) else {
        unreachable!();
    };
    evidence.started_at_ns = 3_000;
    evidence.reconciliation_restored = false;
    evidence = evidence.sealed();
    assert!(matches!(
        owner.apply(&CertificationCommand::RecordRecoveryDrill {
            command_id: id(2),
            evidence,
            recorded_at_ns: 3_200,
        }),
        Err(Error::Recovery)
    ));
}

#[test]
fn forbidden_contract_privilege_never_registers() {
    let mut value = campaign();
    value
        .adapter_contract
        .privilege_policy
        .cluster_admin_allowed = true;
    value.adapter_contract.privilege_policy = value.adapter_contract.privilege_policy.sealed();
    value.adapter_contract = value.adapter_contract.sealed();
    value = value.sealed(&policy());
    let mut owner = DeploymentAdapterCertification::new(policy()).expect("owner");
    assert!(matches!(
        owner.apply(&CertificationCommand::Register {
            command_id: id(80),
            campaign: Box::new(value),
            recorded_at_ns: 3_000,
        }),
        Err(Error::Campaign)
    ));
}

#[test]
fn canonical_report_detects_corruption_and_refuses_replacement() {
    let report = run(&complete_commands())
        .snapshot()
        .last_report
        .expect("report");
    let directory = tempdir().expect("directory");
    let path = directory.path().join("report.bin");
    write_report_create_new(&path, &report).expect("write");
    assert_eq!(read_report(&path).expect("read"), report);
    assert!(matches!(
        write_report_create_new(&path, &report),
        Err(AdapterCertificationReportFileError::Io(_))
    ));
    let mut bytes = std::fs::read(&path).expect("bytes");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, bytes).expect("corrupt");
    assert!(matches!(
        read_report(&path),
        Err(AdapterCertificationReportFileError::Checksum)
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
    let commands = complete_commands();
    let directory = tempdir().expect("directory");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 512 * 1024,
            max_segment_records: 4,
        },
    )
    .expect("writer");
    let recovery = CertificationRecovery {
        certification: DeploymentAdapterCertification::new(policy()).expect("owner"),
        last_sequence: None,
    };
    let mut durable = DurableCertification::new(writer, recovery).expect("durable");
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let expected = durable.certification().snapshot().digest;
    let checkpoint = CertificationCheckpoint {
        sequence: u64::try_from(commands.len() - 1).expect("sequence"),
        certification_digest: expected,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.certification.snapshot().digest, expected);

    let recovery = CertificationRecovery {
        certification: DeploymentAdapterCertification::new(policy()).expect("owner"),
        last_sequence: None,
    };
    let mut failing =
        DurableCertification::new(FailingJournal::default(), recovery).expect("owner");
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(CertificationStorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(CertificationStorageError::Halted(_))
    ));
    assert_eq!(failing.certification().snapshot().accepted_commands, 0);
}

#[test]
fn identical_commands_produce_identical_state_and_report() {
    let commands = complete_commands();
    assert_eq!(run(&commands).snapshot(), run(&commands).snapshot());
}

proptest! {
    #[test]
    fn any_forbidden_privilege_flag_prevents_registration(flag in 0_usize..7) {
        let mut value = campaign();
        let privilege = &mut value.adapter_contract.privilege_policy;
        match flag {
            0 => privilege.credential_material_allowed = true,
            1 => privilege.wildcard_resources_allowed = true,
            2 => privilege.secret_read_allowed = true,
            3 => privilege.cluster_admin_allowed = true,
            4 => privilege.arbitrary_exec_allowed = true,
            5 => privilege.privilege_escalation_allowed = true,
            _ => privilege.cross_region_mutation_allowed = true,
        }
        value.adapter_contract.privilege_policy = privilege.clone().sealed();
        value.adapter_contract = value.adapter_contract.sealed();
        value = value.sealed(&policy());
        let mut owner = DeploymentAdapterCertification::new(policy()).expect("owner");
        let result = owner.apply(&CertificationCommand::Register {
            command_id: id(90), campaign: Box::new(value), recorded_at_ns: 3_000,
        });
        prop_assert!(matches!(result, Err(Error::Campaign)));
    }
}
