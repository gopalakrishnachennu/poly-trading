use super::*;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use tempfile::tempdir;

const CREATED: i64 = 2_000;

const fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> DeploymentPolicy {
    DeploymentPolicy {
        minimum_regions: 2,
        maximum_regions: 8,
        maximum_decisions: 3,
        maximum_fleet_age_ns: 200,
        maximum_rollback_age_ns: 200,
        maximum_package_age_ns: 1_000,
        maximum_decision_age_ns: 500,
        maximum_order_notional_micros: 1_000_000,
        maximum_daily_loss_micros: 100_000,
    }
}

fn seal_fleet(mut fleet: CurrentFleetReadiness) -> CurrentFleetReadiness {
    fleet.current_readiness_digest = [0; 32];
    let serialized = serde_json::to_vec(&fleet).expect("fleet JSON");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"fleet-current-readiness-v1");
    hasher.update(&(serialized.len() as u64).to_le_bytes());
    hasher.update(&serialized);
    fleet.current_readiness_digest = *hasher.finalize().as_bytes();
    fleet
}

fn fleet(observed_at_ns: i64) -> CurrentFleetReadiness {
    seal_fleet(CurrentFleetReadiness {
        campaign_id: bytes(1),
        dossier_id: bytes(2),
        dossier_digest: bytes(3),
        release_digest: bytes(4),
        artifacts_digest: bytes(5),
        rollback_digest: bytes(6),
        completed_regions: vec!["eu-west".to_owned(), "us-east".to_owned()],
        governance_digest: bytes(7),
        observed_at_ns,
        current_readiness_digest: [0; 32],
    })
}

fn region(name: &str, id: u8) -> RegionConfiguration {
    RegionConfiguration {
        region: name.to_owned(),
        environment_digest: bytes(id),
        image_digest: bytes(id + 1),
        configuration_digest: bytes(id + 2),
        infrastructure_plan_digest: bytes(id + 3),
        network_policy_digest: bytes(id + 4),
        observability_digest: bytes(id + 5),
        failover_digest: bytes(id + 6),
        public_admin_enabled: false,
        region_digest: [0; 32],
    }
    .sealed()
}

fn privilege() -> LeastPrivilegePolicy {
    LeastPrivilegePolicy {
        policy_id: bytes(30),
        release_digest: bytes(4),
        artifacts_digest: bytes(5),
        allowed_regions: vec!["eu-west".to_owned(), "us-east".to_owned()],
        allowed_contract_digests: vec![bytes(31), bytes(32)],
        signer_policy_digest: bytes(33),
        maximum_order_notional_micros: 1_000_000,
        maximum_daily_loss_micros: 100_000,
        credential_material_present: false,
        arbitrary_transfer_allowed: false,
        withdrawal_allowed: false,
        contract_upgrade_allowed: false,
        policy_digest: [0; 32],
    }
    .sealed()
}

fn rollback() -> RollbackPackage {
    RollbackPackage {
        rollback_package_id: bytes(40),
        release_digest: bytes(4),
        artifacts_digest: bytes(5),
        rollback_digest: bytes(6),
        rollback_binary_digest: bytes(41),
        rollback_configuration_digest: bytes(42),
        recovery_runbook_digest: bytes(43),
        verification_evidence_digest: bytes(44),
        verified_at_ns: 1_950,
        package_digest: [0; 32],
    }
    .sealed()
}

fn package() -> DeploymentPackage {
    DeploymentPackage {
        deployment_package_id: bytes(50),
        created_at_ns: CREATED,
        expires_at_ns: 2_900,
        fleet: fleet(1_990),
        regions: vec![region("us-east", 60), region("eu-west", 70)],
        least_privilege: privilege(),
        rollback: rollback(),
        policy_digest: [0; 32],
        package_digest: [0; 32],
    }
    .sealed(&policy())
}

fn register(value: DeploymentPackage) -> DeploymentCommand {
    DeploymentCommand::Register {
        command_id: DeploymentCommandId(bytes(80)),
        package: Box::new(value),
        recorded_at_ns: CREATED,
    }
}

fn decision(role: OperatorRole, id: u8, operator: u8, kind: DecisionKind) -> OperatorDecision {
    OperatorDecision {
        decision_id: bytes(id),
        deployment_package_id: bytes(50),
        package_digest: package().package_digest,
        role,
        operator_id: bytes(operator),
        decision: kind,
        reason_digest: (kind == DecisionKind::Reject).then(|| bytes(id + 1)),
        decided_at_ns: 2_010 + i64::from(id - 81) * 10,
        valid_until_ns: 2_500,
        decision_digest: [0; 32],
    }
    .sealed()
}

fn decide(value: OperatorDecision, command: u8) -> DeploymentCommand {
    DeploymentCommand::Decide {
        command_id: DeploymentCommandId(bytes(command)),
        recorded_at_ns: value.decided_at_ns,
        decision: value,
    }
}

fn finalize(at: i64, current_fleet: CurrentFleetReadiness) -> DeploymentCommand {
    DeploymentCommand::Finalize {
        command_id: DeploymentCommandId(bytes(90)),
        deployment_package_id: bytes(50),
        report_id: bytes(91),
        current_fleet,
        evaluated_at_ns: at,
        recorded_at_ns: at,
    }
}

fn successful_commands() -> Vec<DeploymentCommand> {
    vec![
        register(package()),
        decide(
            decision(OperatorRole::Release, 81, 101, DecisionKind::Approve),
            81,
        ),
        decide(
            decision(OperatorRole::Risk, 82, 102, DecisionKind::Approve),
            82,
        ),
        decide(
            decision(OperatorRole::Operations, 83, 103, DecisionKind::Approve),
            83,
        ),
        finalize(2_100, fleet(2_090)),
    ]
}

fn run(commands: &[DeploymentCommand]) -> DeploymentPreflight {
    let mut preflight = DeploymentPreflight::new(policy()).expect("preflight");
    for command in commands {
        preflight.apply(command).expect("command");
    }
    preflight
}

#[test]
fn complete_three_role_preflight_is_ready_but_grants_no_authority() {
    let report = run(&successful_commands())
        .snapshot()
        .last_report
        .expect("report");
    assert_eq!(report.status, PreflightStatus::ReadyForManualDeployment);
    assert!(report.reasons.is_empty());
    assert_eq!(report.distinct_operator_count, 3);
    assert_eq!(report.approved_roles, OperatorRole::ALL);
    assert!(report.manual_operator_execution_required);
    assert!(!report.credential_material_created);
    assert!(!report.signing_authority_granted);
    assert!(!report.deployment_authority_granted);
    assert!(!report.rollback_execution_authority_granted);
    assert!(!report.cloud_control_authority_granted);
    assert!(!report.live_trading_authority_granted);
    assert!(report.verify_digest());
}

#[test]
fn region_omission_public_admin_and_substitution_fail_closed() {
    let mut missing = package();
    missing.regions.pop();
    missing = missing.sealed(&policy());
    let mut preflight = DeploymentPreflight::new(policy()).expect("preflight");
    assert!(matches!(
        preflight.apply(&register(missing)),
        Err(Error::Region)
    ));

    let mut admin = package();
    admin.regions[0].public_admin_enabled = true;
    admin.regions[0] = admin.regions[0].clone().sealed();
    admin = admin.sealed(&policy());
    let mut preflight = DeploymentPreflight::new(policy()).expect("preflight");
    assert!(matches!(
        preflight.apply(&register(admin)),
        Err(Error::Region)
    ));
}

#[test]
fn every_privilege_escalation_is_rejected() {
    for field in 0..4 {
        let mut value = package();
        match field {
            0 => value.least_privilege.credential_material_present = true,
            1 => value.least_privilege.arbitrary_transfer_allowed = true,
            2 => value.least_privilege.withdrawal_allowed = true,
            _ => value.least_privilege.contract_upgrade_allowed = true,
        }
        value.least_privilege = value.least_privilege.clone().sealed();
        value = value.sealed(&policy());
        let mut preflight = DeploymentPreflight::new(policy()).expect("preflight");
        assert!(matches!(
            preflight.apply(&register(value)),
            Err(Error::Privilege)
        ));
    }

    let mut excessive = package();
    excessive.least_privilege.maximum_order_notional_micros = 1_000_001;
    excessive.least_privilege.maximum_daily_loss_micros = 100_001;
    excessive.least_privilege = excessive.least_privilege.clone().sealed();
    excessive = excessive.sealed(&policy());
    let mut preflight = DeploymentPreflight::new(policy()).expect("preflight");
    assert!(matches!(
        preflight.apply(&register(excessive)),
        Err(Error::Privilege)
    ));
}

#[test]
fn rollback_subject_substitution_or_staleness_halts() {
    let mut value = package();
    value.rollback.rollback_digest = bytes(99);
    value.rollback = value.rollback.clone().sealed();
    value = value.sealed(&policy());
    let mut preflight = DeploymentPreflight::new(policy()).expect("preflight");
    assert!(matches!(
        preflight.apply(&register(value)),
        Err(Error::Rollback)
    ));

    let mut stale = package();
    stale.rollback.verified_at_ns = 1_700;
    stale.rollback = stale.rollback.clone().sealed();
    stale = stale.sealed(&policy());
    let mut preflight = DeploymentPreflight::new(policy()).expect("preflight");
    assert!(matches!(
        preflight.apply(&register(stale)),
        Err(Error::Rollback)
    ));
}

#[test]
fn missing_rejected_expired_and_same_operator_decisions_are_attributable() {
    let commands = vec![
        register(package()),
        decide(
            decision(OperatorRole::Release, 81, 101, DecisionKind::Approve),
            81,
        ),
        decide(
            decision(OperatorRole::Risk, 82, 101, DecisionKind::Reject),
            82,
        ),
        finalize(2_501, fleet(2_490)),
    ];
    let report = run(&commands).snapshot().last_report.expect("report");
    assert_eq!(report.status, PreflightStatus::NotReady);
    assert!(report
        .reasons
        .contains(&PreflightReason::MissingRole(OperatorRole::Operations)));
    assert!(report
        .reasons
        .contains(&PreflightReason::ExpiredDecision(OperatorRole::Release)));
    assert!(report
        .reasons
        .contains(&PreflightReason::ExpiredDecision(OperatorRole::Risk)));
    assert!(report
        .reasons
        .contains(&PreflightReason::RejectedRole(OperatorRole::Risk)));
    assert!(report
        .reasons
        .contains(&PreflightReason::OperatorsNotDistinct));
}

#[test]
fn changed_or_stale_current_fleet_cannot_preserve_readiness() {
    let mut commands = successful_commands();
    let mut changed = fleet(2_090);
    changed.governance_digest = bytes(99);
    changed = seal_fleet(changed);
    commands.pop();
    commands.push(finalize(2_100, changed));
    let mut preflight = DeploymentPreflight::new(policy()).expect("preflight");
    for command in &commands[..commands.len() - 1] {
        preflight.apply(command).expect("setup");
    }
    assert!(matches!(
        preflight.apply(commands.last().expect("final")),
        Err(Error::Fleet)
    ));

    let mut commands = successful_commands();
    commands.pop();
    commands.push(finalize(2_300, fleet(2_090)));
    let report = run(&commands).snapshot().last_report.expect("report");
    assert!(report
        .reasons
        .contains(&PreflightReason::FleetReadinessStale));
}

#[test]
fn report_file_is_create_new_canonical_and_corruption_detecting() {
    let report = run(&successful_commands())
        .snapshot()
        .last_report
        .expect("report");
    let directory = tempdir().expect("dir");
    let path = directory.path().join("report.bin");
    write_report_create_new(&path, &report).expect("write");
    assert_eq!(read_report(&path).expect("read"), report);
    assert!(write_report_create_new(&path, &report).is_err());

    let noncanonical_path = directory.path().join("noncanonical.bin");
    let mut noncanonical = std::fs::read(&path).expect("bytes");
    noncanonical.truncate(noncanonical.len() - 32);
    noncanonical.insert(24, b' ');
    let body_len = u64::try_from(noncanonical.len() - 24).expect("length");
    noncanonical[16..24].copy_from_slice(&body_len.to_le_bytes());
    let checksum = blake3::hash(&noncanonical);
    noncanonical.extend_from_slice(checksum.as_bytes());
    std::fs::write(&noncanonical_path, noncanonical).expect("fixture");
    assert!(matches!(
        read_report(&noncanonical_path),
        Err(PreflightReportFileError::NonCanonical)
    ));

    let mut corrupt = std::fs::read(&path).expect("bytes");
    corrupt[30] ^= 1;
    std::fs::write(&path, corrupt).expect("corrupt fixture");
    assert!(matches!(
        read_report(&path),
        Err(PreflightReportFileError::Checksum)
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
            max_segment_records: 2,
        },
    )
    .expect("writer");
    let recovery = DeploymentRecovery {
        preflight: DeploymentPreflight::new(policy()).expect("preflight"),
        last_sequence: None,
    };
    let mut durable = DurableDeploymentPreflight::new(writer, recovery).expect("durable");
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let expected = durable.preflight().snapshot().digest;
    let checkpoint = DeploymentCheckpoint {
        sequence: u64::try_from(commands.len() - 1).expect("sequence"),
        preflight_digest: expected,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.preflight.snapshot().digest, expected);

    let recovery = DeploymentRecovery {
        preflight: DeploymentPreflight::new(policy()).expect("preflight"),
        last_sequence: None,
    };
    let mut failing =
        DurableDeploymentPreflight::new(FailingJournal::default(), recovery).expect("new");
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(DeploymentStorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(DeploymentStorageError::Halted(_))
    ));
    assert_eq!(failing.preflight().snapshot().accepted_commands, 0);
}

#[test]
fn identical_commands_produce_identical_complete_state() {
    let first = run(&successful_commands());
    let second = run(&successful_commands());
    assert_eq!(first.snapshot().digest, second.snapshot().digest);
    assert_eq!(first.snapshot().last_report, second.snapshot().last_report);
}

proptest! {
    #[test]
    fn any_escalation_bit_prevents_registration(bits in 1_u8..16) {
        let mut value = package();
        value.least_privilege.credential_material_present = bits & 1 != 0;
        value.least_privilege.arbitrary_transfer_allowed = bits & 2 != 0;
        value.least_privilege.withdrawal_allowed = bits & 4 != 0;
        value.least_privilege.contract_upgrade_allowed = bits & 8 != 0;
        value.least_privilege = value.least_privilege.clone().sealed();
        value = value.sealed(&policy());
        let mut preflight = DeploymentPreflight::new(policy()).expect("preflight");
        prop_assert!(matches!(preflight.apply(&register(value)), Err(Error::Privilege)));
    }
}
