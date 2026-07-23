use super::*;
use deployment_execution_intent::{
    DeploymentOperation, ExecutionReportStatus, IsolatedExecutorContract, PrivilegeCeiling,
};
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use production_change_readiness::{
    ProductionChangeSubject, ProductionReadinessRecord, ReadinessStatus,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(value: u8) -> [u8; 32] {
    [value; 32]
}
fn policy() -> ExecutorSessionPolicy {
    ExecutorSessionPolicy {
        maximum_report_age_ns: 1_000,
        maximum_session_duration_ns: 1_000,
        maximum_lease_lifetime_ns: 200,
        maximum_heartbeat_gap_ns: 100,
        maximum_request_lifetime_ns: 50,
        maximum_requests: 8,
    }
}
fn upstream_policy() -> ExecutionIntentPolicy {
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

fn readiness(subject: &ProductionChangeSubject) -> ProductionReadinessRecord {
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

fn upstream() -> (ExecutionIntentPlan, ExecutionCertificationReport) {
    let subject = subject();
    let ceiling = PrivilegeCeiling {
        allowed_operations: vec![
            DeploymentOperation::ApplyConfiguration,
            DeploymentOperation::VerifyHealth,
        ],
        allowed_regions: vec!["us-east-1".into(), "us-west-2".into()],
        allowed_resource_digests: vec![id(3), id(4)],
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
    let steps = vec![
        deployment_execution_intent::ExecutionStep {
            index: 0,
            region: "us-east-1".into(),
            operation: DeploymentOperation::ApplyConfiguration,
            resource_digest: id(3),
            step_digest: [0; 32],
        }
        .sealed(),
        deployment_execution_intent::ExecutionStep {
            index: 1,
            region: "us-west-2".into(),
            operation: DeploymentOperation::VerifyHealth,
            resource_digest: id(4),
            step_digest: [0; 32],
        }
        .sealed(),
    ];
    let plan = ExecutionIntentPlan {
        plan_id: id(30),
        created_at_ns: 1_100,
        expires_at_ns: 1_180,
        readiness_valid_until_ns: 11_000,
        readiness_record: readiness(&subject),
        subject,
        executor_contract: contract,
        steps,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&upstream_policy());
    let report = ExecutionCertificationReport {
        report_id: id(31),
        plan_digest: plan.plan_digest,
        readiness_record_digest: plan.readiness_record.record_digest,
        subject_digest: plan.subject.subject_digest,
        contract_digest: plan.executor_contract.contract_digest,
        dry_run_chain_digest: id(32),
        completed_step_count: 2,
        finalized_at_ns: 1_200,
        status: ExecutionReportStatus::SimulatedHandoffsCompleted,
        manual_execution_still_required: true,
        credential_material_created: false,
        signature_authority_granted: false,
        authenticated_transport_granted: false,
        deployment_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed();
    (plan, report)
}

fn plan() -> ExecutorSessionPlan {
    let (execution_plan, execution_report) = upstream();
    let templates = execution_plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            ExecutorRequestTemplate {
                sequence: u32::try_from(index).unwrap(),
                region: step.region.clone(),
                operation: step.operation,
                resource_digest: step.resource_digest,
                payload_digest: id(40 + u8::try_from(index).unwrap()),
                template_digest: [0; 32],
            }
            .sealed()
        })
        .collect();
    ExecutorSessionPlan {
        plan_id: id(50),
        created_at_ns: 1_250,
        expires_at_ns: 2_000,
        upstream_policy: upstream_policy(),
        execution_plan,
        execution_report,
        isolation_contract: ProcessIsolationContract {
            runtime_binary_digest: id(51),
            sandbox_profile_digest: id(52),
            audit_schema_digest: id(53),
            network_access: false,
            credential_access: false,
            signing_access: false,
            privileged_process: false,
            arbitrary_shell: false,
            filesystem_escape: false,
            host_namespace_access: false,
            isolation_digest: [0; 32],
        }
        .sealed(),
        request_templates: templates,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn register() -> SessionCommand {
    SessionCommand::Register {
        command_id: SessionCommandId(id(60)),
        plan: Box::new(plan()),
        recorded_at_ns: 1_250,
    }
}
fn lease(command: u8, lease_id: u8, at: i64, expires: i64) -> SessionCommand {
    SessionCommand::AcquireLease {
        command_id: SessionCommandId(id(command)),
        lease_id: id(lease_id),
        owner_label_digest: id(62),
        acquired_at_ns: at,
        requested_expires_at_ns: expires,
        recorded_at_ns: at,
    }
}

fn open(owner: &mut ExecutorSessionSimulator) {
    owner.apply(&register()).unwrap();
    owner.apply(&lease(61, 70, 1_260, 1_450)).unwrap();
    owner
        .apply(&SessionCommand::OpenSession {
            command_id: SessionCommandId(id(63)),
            session_id: id(71),
            process_instance_digest: id(72),
            opened_at_ns: 1_261,
            recorded_at_ns: 1_261,
        })
        .unwrap();
    owner
        .apply(&SessionCommand::Heartbeat {
            command_id: SessionCommandId(id(74)),
            lease_id: id(70),
            sequence: 0,
            observed_at_ns: 1_262,
            process_healthy: true,
            journal_healthy: true,
            reconciliation_healthy: true,
            clock_healthy: true,
            recorded_at_ns: 1_262,
        })
        .unwrap();
}

fn issue(
    owner: &mut ExecutorSessionSimulator,
    command: u8,
    request: u8,
    at: i64,
) -> ExecutorRequestEnvelope {
    match owner
        .apply(&SessionCommand::IssueRequest {
            command_id: SessionCommandId(id(command)),
            request_id: id(request),
            issued_at_ns: at,
            requested_expires_at_ns: at + 30,
            recorded_at_ns: at,
        })
        .unwrap()
        .detail
    {
        SessionDetail::RequestIssued(value) => *value,
        _ => panic!("request"),
    }
}

fn observation(
    request: &ExecutorRequestEnvelope,
    kind: SimulatedObservationKind,
    at: i64,
) -> SimulatedExecutorObservation {
    SimulatedExecutorObservation {
        request_id: request.request_id,
        request_digest: request.request_digest,
        kind,
        observed_at_ns: at,
        source_fixture_digest: id(90),
        simulated_only: true,
        credential_loaded: false,
        signature_produced: false,
        authenticated_request_sent: false,
        external_submission_observed: false,
        external_mutation_observed: false,
        observation_digest: [0; 32],
    }
    .sealed()
}

#[test]
fn acknowledged_and_unknown_requests_complete_only_after_reconciliation() {
    let mut owner = ExecutorSessionSimulator::new(policy()).unwrap();
    open(&mut owner);
    let first = issue(&mut owner, 64, 80, 1_270);
    owner
        .apply(&SessionCommand::Observe {
            command_id: SessionCommandId(id(65)),
            observation: observation(&first, SimulatedObservationKind::Acknowledged, 1_271),
            recorded_at_ns: 1_271,
        })
        .unwrap();
    let second = issue(&mut owner, 66, 81, 1_280);
    owner
        .apply(&SessionCommand::Observe {
            command_id: SessionCommandId(id(67)),
            observation: observation(&second, SimulatedObservationKind::Unknown, 1_281),
            recorded_at_ns: 1_281,
        })
        .unwrap();
    assert_eq!(
        owner.snapshot().status,
        Some(SessionStatus::ReconciliationRequired)
    );
    assert_eq!(owner.snapshot().resolved_requests, 1);
    let evidence = NoMutationReconciliation {
        request_id: Some(second.request_id),
        prior_state_digest: owner.snapshot().digest,
        durable_state_digest: id(91),
        external_state_digest: id(92),
        reconciled_at_ns: 1_282,
        no_external_mutation: true,
        reconciliation_digest: [0; 32],
    }
    .sealed();
    owner
        .apply(&SessionCommand::Reconcile {
            command_id: SessionCommandId(id(68)),
            evidence,
            recorded_at_ns: 1_282,
        })
        .unwrap();
    owner
        .apply(&SessionCommand::Close {
            command_id: SessionCommandId(id(69)),
            closed_at_ns: 1_283,
            recorded_at_ns: 1_283,
        })
        .unwrap();
    let dossier = match owner
        .apply(&SessionCommand::Finalize {
            command_id: SessionCommandId(id(73)),
            dossier_id: id(93),
            finalized_at_ns: 1_284,
            recorded_at_ns: 1_284,
        })
        .unwrap()
        .detail
    {
        SessionDetail::Finalized(value) => *value,
        _ => panic!("dossier"),
    };
    assert!(dossier.verify_digest());
    assert_eq!(dossier.resolved_request_count, 2);
    assert!(dossier.simulated_only);
    assert!(!dossier.external_submission_authority_granted);
    let dir = tempdir().unwrap();
    let path = dir.path().join("dossier.bin");
    write_dossier_create_new(&path, &dossier).unwrap();
    assert_eq!(read_dossier(&path).unwrap(), dossier);
    assert!(write_dossier_create_new(&path, &dossier).is_err());
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        read_dossier(&path),
        Err(SessionDossierFileError::Checksum)
    ));
}

#[test]
fn substituted_report_and_unsafe_isolation_halt_registration() {
    let mut bad = plan();
    bad.execution_report.contract_digest = id(99);
    bad.execution_report = bad.execution_report.sealed();
    bad = bad.sealed(&policy());
    let mut owner = ExecutorSessionSimulator::new(policy()).unwrap();
    assert_eq!(
        owner.apply(&SessionCommand::Register {
            command_id: SessionCommandId(id(1)),
            plan: Box::new(bad),
            recorded_at_ns: 1_250
        }),
        Err(Error::Plan)
    );
    assert!(owner.is_halted());
    let mut bad = plan();
    bad.isolation_contract.network_access = true;
    bad.isolation_contract = bad.isolation_contract.sealed();
    bad = bad.sealed(&policy());
    let mut owner = ExecutorSessionSimulator::new(policy()).unwrap();
    assert!(matches!(
        owner.apply(&SessionCommand::Register {
            command_id: SessionCommandId(id(1)),
            plan: Box::new(bad),
            recorded_at_ns: 1_250
        }),
        Err(Error::Plan)
    ));
}

#[test]
fn side_effect_claim_and_second_lease_fail_closed() {
    let mut owner = ExecutorSessionSimulator::new(policy()).unwrap();
    open(&mut owner);
    let request = issue(&mut owner, 64, 80, 1_270);
    let mut observed = observation(&request, SimulatedObservationKind::Acknowledged, 1_271);
    observed.external_mutation_observed = true;
    observed = observed.sealed();
    assert_eq!(
        owner.apply(&SessionCommand::Observe {
            command_id: SessionCommandId(id(65)),
            observation: observed,
            recorded_at_ns: 1_271
        }),
        Err(Error::Observation)
    );
    let mut owner = ExecutorSessionSimulator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    owner.apply(&lease(61, 70, 1_260, 1_450)).unwrap();
    assert_eq!(owner.apply(&lease(62, 71, 1_261, 1_400)), Err(Error::Lease));

    let mut owner = ExecutorSessionSimulator::new(policy()).unwrap();
    owner.apply(&register()).unwrap();
    owner.apply(&lease(61, 70, 1_260, 1_450)).unwrap();
    owner
        .apply(&SessionCommand::ExpireDeadMan {
            command_id: SessionCommandId(id(62)),
            observed_at_ns: 1_450,
            recorded_at_ns: 1_450,
        })
        .unwrap();
    assert_eq!(owner.apply(&lease(63, 70, 1_451, 1_600)), Err(Error::Lease));
}

#[test]
fn deadman_and_restart_require_reconciliation_before_new_work() {
    let mut owner = ExecutorSessionSimulator::new(policy()).unwrap();
    open(&mut owner);
    let request = issue(&mut owner, 64, 80, 1_270);
    owner
        .apply(&SessionCommand::ExpireDeadMan {
            command_id: SessionCommandId(id(65)),
            observed_at_ns: 1_450,
            recorded_at_ns: 1_450,
        })
        .unwrap();
    assert_eq!(
        owner.snapshot().status,
        Some(SessionStatus::ReconciliationRequired)
    );
    let evidence = NoMutationReconciliation {
        request_id: Some(request.request_id),
        prior_state_digest: owner.snapshot().digest,
        durable_state_digest: id(91),
        external_state_digest: id(92),
        reconciled_at_ns: 1_451,
        no_external_mutation: true,
        reconciliation_digest: [0; 32],
    }
    .sealed();
    owner
        .apply(&SessionCommand::Reconcile {
            command_id: SessionCommandId(id(66)),
            evidence,
            recorded_at_ns: 1_451,
        })
        .unwrap();
    owner.apply(&lease(67, 71, 1_452, 1_600)).unwrap();
    owner
        .apply(&SessionCommand::Heartbeat {
            command_id: SessionCommandId(id(68)),
            lease_id: id(71),
            sequence: 0,
            observed_at_ns: 1_453,
            process_healthy: true,
            journal_healthy: true,
            reconciliation_healthy: true,
            clock_healthy: true,
            recorded_at_ns: 1_453,
        })
        .unwrap();
    assert!(matches!(
        owner.apply(&SessionCommand::IssueRequest {
            command_id: SessionCommandId(id(69)),
            request_id: id(82),
            issued_at_ns: 1_454,
            requested_expires_at_ns: 1_480,
            recorded_at_ns: 1_454,
        }),
        Ok(SessionOutcome {
            detail: SessionDetail::RequestIssued(_),
            ..
        })
    ));

    let mut owner = ExecutorSessionSimulator::new(policy()).unwrap();
    open(&mut owner);
    let prior = owner.snapshot().digest;
    owner
        .apply(&SessionCommand::Restart {
            command_id: SessionCommandId(id(64)),
            prior_state_digest: prior,
            restarted_at_ns: 1_270,
            recorded_at_ns: 1_270,
        })
        .unwrap();
    assert_eq!(
        owner.snapshot().status,
        Some(SessionStatus::RestartRecoveryRequired)
    );
    let evidence = NoMutationReconciliation {
        request_id: None,
        prior_state_digest: prior,
        durable_state_digest: id(91),
        external_state_digest: id(92),
        reconciled_at_ns: 1_271,
        no_external_mutation: true,
        reconciliation_digest: [0; 32],
    }
    .sealed();
    owner
        .apply(&SessionCommand::Reconcile {
            command_id: SessionCommandId(id(65)),
            evidence,
            recorded_at_ns: 1_271,
        })
        .unwrap();
    assert_eq!(owner.snapshot().status, Some(SessionStatus::Paused));
}

#[test]
fn command_idempotency_and_checkpoint_corruption_are_strict() {
    let mut owner = ExecutorSessionSimulator::new(policy()).unwrap();
    let command = register();
    let first = owner.apply(&command).unwrap();
    assert_eq!(owner.apply(&command).unwrap(), first);
    assert_eq!(owner.snapshot().accepted_commands, 1);
    let checkpoint = ExecutorSessionCheckpoint {
        sequence: 0,
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
fn durable_replay_and_sync_failure_are_fail_closed() {
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
    let recovery = ExecutorSessionRecovery {
        owner: ExecutorSessionSimulator::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableExecutorSession::new(writer, recovery).unwrap();
    durable.apply(&register()).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let recovered = recover_segmented(
        &segments,
        policy(),
        Some(ExecutorSessionCheckpoint {
            sequence: 0,
            state_digest: expected,
        }),
    )
    .unwrap();
    assert_eq!(recovered.owner.snapshot().digest, expected);

    let recovery = ExecutorSessionRecovery {
        owner: ExecutorSessionSimulator::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableExecutorSession::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&register()),
        Err(ExecutorSessionStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&register()),
        Err(ExecutorSessionStorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn lease_never_exceeds_policy(requested in 1_i64..500) {
        let mut owner = ExecutorSessionSimulator::new(policy()).unwrap(); owner.apply(&register()).unwrap();
        let result = owner.apply(&lease(61, 70, 1_260, 1_260 + requested));
        prop_assert_eq!(result.is_ok(), requested <= policy().maximum_lease_lifetime_ns);
    }
}
