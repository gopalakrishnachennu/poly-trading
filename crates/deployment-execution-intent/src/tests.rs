use super::*;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use production_change_readiness::{ProductionReadinessRecord, ReadinessStatus};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(v: u8) -> [u8; 32] {
    [v; 32]
}
fn policy() -> ExecutionIntentPolicy {
    ExecutionIntentPolicy {
        maximum_readiness_age_ns: 10_000,
        maximum_plan_age_ns: 5_000,
        maximum_intent_lifetime_ns: 100,
        maximum_steps: 8,
        maximum_regions: 4,
        maximum_resources: 16,
    }
}

fn subject() -> ProductionChangeSubject {
    ProductionChangeSubject {
        release_digest: id(1),
        binary_digest: id(2),
        configuration_digest: id(3),
        infrastructure_digest: id(4),
        observability_digest: id(5),
        plan_digests: vec![id(6)],
        certificate_digests: vec![id(7)],
        preflight_report_digests: vec![id(8)],
        rollback_package_digests: vec![id(9)],
        subject_digest: [0; 32],
    }
    .sealed()
}

fn record(subject: &ProductionChangeSubject) -> ProductionReadinessRecord {
    ProductionReadinessRecord {
        record_id: id(10),
        candidate_id: id(11),
        candidate_digest: id(12),
        subject_digest: subject.subject_digest,
        finalized_at_ns: 1_000,
        status: ReadinessStatus::ProductionChangeReady,
        reasons: vec![],
        eligible_campaign_count: 2,
        case_count: 8,
        independent_plan_count: 8,
        restart_count: 2,
        approval_set_count: 8,
        manifest_diversity: 2,
        schedule_diversity: 2,
        result_chain_diversity: 2,
        plan_diversity: 8,
        regression_campaign_floor: 2,
        regression_case_floor: 8,
        regression_independent_plan_floor: 8,
        regression_restart_floor: 2,
        regression_approval_set_floor: 8,
        operator_execution_required: true,
        credential_material_created: false,
        authentication_authority_granted: false,
        deployment_authority_granted: false,
        rollback_execution_authority_granted: false,
        traffic_authority_granted: false,
        cloud_control_authority_granted: false,
        live_trading_authority_granted: false,
        record_digest: [0; 32],
    }
    .sealed()
}

fn plan() -> ExecutionIntentPlan {
    let subject = subject();
    let ceiling = PrivilegeCeiling {
        allowed_operations: vec![
            DeploymentOperation::ApplyConfiguration,
            DeploymentOperation::VerifyHealth,
        ],
        allowed_regions: vec!["us-east-1".into(), "us-west-2".into()],
        allowed_resource_digests: vec![subject.configuration_digest, subject.infrastructure_digest],
        wildcard_access: false,
        secret_read: false,
        cluster_admin: false,
        arbitrary_exec: false,
        privilege_escalation: false,
        cross_region_mutation: false,
        credential_loading: false,
        ceiling_digest: [0; 32],
    }
    .sealed();
    let contract = IsolatedExecutorContract {
        executor_binary_digest: id(20),
        executor_schema_digest: id(21),
        audit_policy_digest: id(22),
        subject_digest: subject.subject_digest,
        privilege_ceiling: ceiling,
        credential_loading: false,
        signature_production: false,
        authenticated_transport: false,
        external_submission: false,
        contract_digest: [0; 32],
    }
    .sealed();
    ExecutionIntentPlan {
        plan_id: id(30),
        created_at_ns: 1_100,
        expires_at_ns: 2_000,
        readiness_valid_until_ns: 11_000,
        readiness_record: record(&subject),
        subject,
        executor_contract: contract,
        steps: vec![
            ExecutionStep {
                index: 0,
                region: "us-east-1".into(),
                operation: DeploymentOperation::ApplyConfiguration,
                resource_digest: id(3),
                step_digest: [0; 32],
            }
            .sealed(),
            ExecutionStep {
                index: 1,
                region: "us-west-2".into(),
                operation: DeploymentOperation::VerifyHealth,
                resource_digest: id(4),
                step_digest: [0; 32],
            }
            .sealed(),
        ],
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn register() -> ExecutionCommand {
    ExecutionCommand::Register {
        command_id: ExecutionCommandId(id(40)),
        plan: Box::new(plan()),
        recorded_at_ns: 1_100,
    }
}
fn dry_run(index: usize) -> ExecutionCommand {
    let case = DryRunCase::ALL[index];
    let sequence = u8::try_from(index).unwrap();
    let offset = i64::try_from(index).unwrap();
    ExecutionCommand::RecordDryRun {
        command_id: ExecutionCommandId(id(50 + sequence)),
        evidence: ExecutorDryRunEvidence {
            sequence,
            case,
            expected: case.expected(),
            observed: case.expected(),
            observed_at_ns: 1_110 + offset,
            observation_digest: id(80 + sequence),
            credential_loaded: false,
            signature_produced: false,
            authenticated_request_sent: false,
            external_mutation_observed: false,
            evidence_digest: [0; 32],
        }
        .sealed(),
        recorded_at_ns: 1_110 + offset,
    }
}

fn certify(owner: &mut DeploymentExecutionIntent) {
    owner.apply(&register()).unwrap();
    for i in 0..DryRunCase::ALL.len() {
        owner.apply(&dry_run(i)).unwrap();
    }
    owner
        .apply(&ExecutionCommand::Certify {
            command_id: ExecutionCommandId(id(70)),
            plan_id: id(30),
            certified_at_ns: 1_130,
            recorded_at_ns: 1_130,
        })
        .unwrap();
}

fn issue(
    owner: &mut DeploymentExecutionIntent,
    command: u8,
    intent: u8,
    at: i64,
) -> ManualExecutionIntent {
    match owner
        .apply(&ExecutionCommand::IssueIntent {
            command_id: ExecutionCommandId(id(command)),
            intent_id: id(intent),
            issued_at_ns: at,
            requested_expires_at_ns: at + 50,
            recorded_at_ns: at,
        })
        .unwrap()
        .detail
    {
        ExecutionDetail::IntentIssued(value) => *value,
        _ => panic!("intent"),
    }
}

#[test]
fn complete_matrix_allows_only_ordered_one_use_simulated_handoffs() {
    let mut owner = DeploymentExecutionIntent::new(policy()).unwrap();
    certify(&mut owner);
    for (index, (command, intent_id, at)) in
        [(71, 91, 1_140), (73, 92, 1_160)].into_iter().enumerate()
    {
        let intent = issue(&mut owner, command, intent_id, at);
        assert_eq!(intent.step.index as usize, index);
        assert!(intent.manual_operator_required);
        assert!(!intent.deployment_authority_granted);
        owner
            .apply(&ExecutionCommand::ConsumeIntent {
                command_id: ExecutionCommandId(id(command + 1)),
                intent: Box::new(intent),
                operator_handoff_digest: id(99),
                consumed_at_ns: at + 1,
                recorded_at_ns: at + 1,
            })
            .unwrap();
    }
    let report = match owner
        .apply(&ExecutionCommand::Finalize {
            command_id: ExecutionCommandId(id(75)),
            report_id: id(100),
            finalized_at_ns: 1_180,
            recorded_at_ns: 1_180,
        })
        .unwrap()
        .detail
    {
        ExecutionDetail::Finalized(v) => *v,
        _ => panic!("report"),
    };
    assert!(report.verify_digest());
    assert_eq!(report.completed_step_count, 2);
    assert!(report.manual_execution_still_required);
    assert!(!report.credential_material_created);
    assert!(!report.deployment_authority_granted);
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
        Err(ExecutionReportFileError::Checksum)
    ));
}

#[test]
fn readiness_substitution_and_privilege_escape_halt() {
    let mut bad = plan();
    bad.subject.configuration_digest = id(44);
    bad = bad.sealed(&policy());
    let mut owner = DeploymentExecutionIntent::new(policy()).unwrap();
    assert_eq!(
        owner.apply(&ExecutionCommand::Register {
            command_id: ExecutionCommandId(id(1)),
            plan: Box::new(bad),
            recorded_at_ns: 1_100
        }),
        Err(Error::Plan)
    );
    assert!(owner.is_halted());

    let mut bad = plan();
    bad.executor_contract.privilege_ceiling.wildcard_access = true;
    bad.executor_contract.privilege_ceiling = bad.executor_contract.privilege_ceiling.sealed();
    bad.executor_contract = bad.executor_contract.sealed();
    bad = bad.sealed(&policy());
    let mut owner = DeploymentExecutionIntent::new(policy()).unwrap();
    assert!(matches!(
        owner.apply(&ExecutionCommand::Register {
            command_id: ExecutionCommandId(id(1)),
            plan: Box::new(bad),
            recorded_at_ns: 1_100
        }),
        Err(Error::Plan)
    ));
}

#[test]
fn dry_run_side_effect_incomplete_certification_and_expired_intent_fail_closed() {
    let mut owner = DeploymentExecutionIntent::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    assert_eq!(
        owner.apply(&ExecutionCommand::Certify {
            command_id: ExecutionCommandId(id(2)),
            plan_id: id(30),
            certified_at_ns: 1_111,
            recorded_at_ns: 1_111
        }),
        Err(Error::DryRun)
    );

    let mut owner = DeploymentExecutionIntent::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    let mut command = dry_run(0);
    if let ExecutionCommand::RecordDryRun { evidence, .. } = &mut command {
        evidence.credential_loaded = true;
        *evidence = evidence.clone().sealed();
    }
    assert_eq!(owner.apply(&command), Err(Error::DryRun));
    assert!(owner.is_halted());

    let mut owner = DeploymentExecutionIntent::new(policy()).unwrap();
    certify(&mut owner);
    let intent = issue(&mut owner, 71, 91, 1_140);
    assert_eq!(
        owner.apply(&ExecutionCommand::ConsumeIntent {
            command_id: ExecutionCommandId(id(72)),
            intent: Box::new(intent),
            operator_handoff_digest: id(99),
            consumed_at_ns: 1_191,
            recorded_at_ns: 1_191
        }),
        Err(Error::Intent)
    );
}

#[test]
fn command_is_content_idempotent_but_equivocation_halts() {
    let mut owner = DeploymentExecutionIntent::new(policy()).unwrap();
    let command = register();
    let first = owner.apply(&command).unwrap();
    assert_eq!(owner.apply(&command).unwrap(), first);
    assert_eq!(owner.snapshot().accepted_commands, 1);
    let mut changed = register();
    if let ExecutionCommand::Register { recorded_at_ns, .. } = &mut changed {
        *recorded_at_ns += 1;
    }
    assert_eq!(owner.apply(&changed), Err(Error::IdempotencyConflict));
    assert!(owner.is_halted());
}

#[test]
fn corrupted_report_and_checkpoint_are_rejected() {
    let mut owner = DeploymentExecutionIntent::new(policy()).unwrap();
    certify(&mut owner);
    let checkpoint = ExecutionCheckpoint {
        sequence: 11,
        state_digest: owner.snapshot().digest,
    };
    let dir = tempdir().unwrap();
    let path = dir.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&path).unwrap(), checkpoint);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[24] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(read_checkpoint(&path).is_err());
}

#[test]
fn durable_register_replays_to_identical_state() {
    let directory = tempdir().unwrap();
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 4 * 1024 * 1024,
            max_segment_records: 2,
        },
    )
    .unwrap();
    let recovery = ExecutionRecovery {
        owner: DeploymentExecutionIntent::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableExecutionIntent::new(writer, recovery).unwrap();
    durable.apply(&register()).unwrap();
    let expected = durable.owner().snapshot().digest;
    let checkpoint = ExecutionCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    drop(durable);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).unwrap();
    assert_eq!(recovered.owner.snapshot().digest, expected);
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
fn sync_failure_never_installs_state_and_poisons_owner() {
    let recovery = ExecutionRecovery {
        owner: DeploymentExecutionIntent::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableExecutionIntent::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        durable.apply(&register()),
        Err(ExecutionStorageError::Journal(_))
    ));
    assert_eq!(durable.owner().snapshot().accepted_commands, 0);
    assert!(matches!(
        durable.apply(&register()),
        Err(ExecutionStorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn intent_lifetime_never_exceeds_policy(requested in 1_i64..500) {
        let mut owner = DeploymentExecutionIntent::new(policy()).unwrap(); certify(&mut owner);
        let result = owner.apply(&ExecutionCommand::IssueIntent { command_id: ExecutionCommandId(id(71)), intent_id: id(91), issued_at_ns: 1_140, requested_expires_at_ns: 1_140 + requested, recorded_at_ns: 1_140 });
        prop_assert_eq!(result.is_ok(), requested <= policy().maximum_intent_lifetime_ns);
    }
}
