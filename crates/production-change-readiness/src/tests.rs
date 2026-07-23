use super::*;
use deployment_change_campaign::{CampaignEvidenceReason, ChangeCampaignEvidence};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> ProductionReadinessPolicy {
    ProductionReadinessPolicy {
        maximum_evidence_records: 8,
        minimum_eligible_campaigns: 2,
        minimum_manifest_diversity: 2,
        minimum_schedule_diversity: 2,
        minimum_result_chain_diversity: 2,
        minimum_plan_diversity: 8,
        minimum_case_count: 8,
        minimum_independent_plan_count: 8,
        minimum_restart_count: 2,
        minimum_approval_set_count: 8,
        retention_basis_points: 9_000,
        maximum_evidence_age_ns: 2_000,
        maximum_candidate_age_ns: 2_000,
        maximum_decision_age_ns: 1_000,
    }
}

fn evidence(seed: u8, evaluated_at_ns: i64) -> ChangeCampaignEvidence {
    let mut value = ChangeCampaignEvidence {
        evidence_id: id(seed),
        campaign_id: id(seed + 1),
        manifest_digest: id(seed + 2),
        evaluated_at_ns,
        status: CampaignEvidenceStatus::OperatorReviewEligible,
        reasons: Vec::new(),
        required_scenarios: RequiredScenario::ALL.to_vec(),
        covered_scenarios: RequiredScenario::ALL.to_vec(),
        case_count: 4,
        completed_case_count: 4,
        independent_plan_count: 4,
        case_schedule_digest: id(seed + 3),
        case_result_chain_digest: id(seed + 4),
        restart_reconstruction_count: 1,
        approval_set_count: 4,
        plan_digests: (0..4).map(|index| id(seed + 10 + index)).collect(),
        certificate_digests: vec![id(seed + 20)],
        preflight_report_digests: vec![id(seed + 21)],
        rollback_package_digests: vec![id(seed + 22)],
        operator_decision_required: true,
        credential_material_created: false,
        authentication_authority_granted: false,
        deployment_authority_granted: false,
        rollback_execution_authority_granted: false,
        traffic_authority_granted: false,
        cloud_control_authority_granted: false,
        live_trading_authority_granted: false,
        evidence_digest: [0; 32],
    };
    value.evidence_digest = campaign_evidence_digest(&value);
    value
}

fn campaign_evidence_digest(value: &ChangeCampaignEvidence) -> [u8; 32] {
    let mut clone = value.clone();
    clone.evidence_digest = [0; 32];
    digest_json(b"deployment-change-campaign-evidence-v2", &clone)
}

fn baseline() -> ReadinessBaseline {
    ReadinessBaseline {
        campaign_count: 2,
        case_count: 8,
        independent_plan_count: 8,
        restart_count: 2,
        approval_set_count: 8,
        baseline_digest: [0; 32],
    }
    .sealed()
}

fn subject(evidence: &[ChangeCampaignEvidence]) -> ProductionChangeSubject {
    ProductionChangeSubject {
        release_digest: id(200),
        binary_digest: id(201),
        configuration_digest: id(202),
        infrastructure_digest: id(203),
        observability_digest: id(204),
        plan_digests: union_subjects(evidence, |item| &item.plan_digests),
        certificate_digests: union_subjects(evidence, |item| &item.certificate_digests),
        preflight_report_digests: union_subjects(evidence, |item| &item.preflight_report_digests),
        rollback_package_digests: union_subjects(evidence, |item| &item.rollback_package_digests),
        subject_digest: [0; 32],
    }
    .sealed()
}

fn candidate_with(evidence: Vec<ChangeCampaignEvidence>) -> ProductionReadinessCandidate {
    ProductionReadinessCandidate {
        candidate_id: id(210),
        created_at_ns: 10_000,
        expires_at_ns: 11_000,
        subject: subject(&evidence),
        evidence,
        baseline: baseline(),
        policy_digest: [0; 32],
        candidate_digest: [0; 32],
    }
    .sealed(&policy())
}

fn candidate() -> ProductionReadinessCandidate {
    candidate_with(vec![evidence(1, 9_900), evidence(40, 9_901)])
}

fn decision(
    candidate: &ProductionReadinessCandidate,
    role: DecisionRole,
    command: u8,
    operator: u8,
    value: DecisionValue,
    at: i64,
    valid_until: i64,
) -> ReadinessCommand {
    ReadinessCommand::RecordDecision {
        command_id: ReadinessCommandId(id(command)),
        decision: ReadinessDecision {
            decision_id: id(command + 50),
            candidate_id: candidate.candidate_id,
            candidate_digest: candidate.candidate_digest,
            subject_digest: candidate.subject.subject_digest,
            role,
            operator_id: id(operator),
            value,
            decided_at_ns: at,
            valid_until_ns: valid_until,
            decision_digest: [0; 32],
        }
        .sealed(),
        recorded_at_ns: at,
    }
}

fn commands_for(candidate: &ProductionReadinessCandidate) -> Vec<ReadinessCommand> {
    vec![
        ReadinessCommand::Register {
            command_id: ReadinessCommandId(id(1)),
            recorded_at_ns: candidate.created_at_ns,
            candidate: Box::new(candidate.clone()),
        },
        decision(
            candidate,
            DecisionRole::Release,
            2,
            11,
            DecisionValue::Approve,
            10_010,
            10_800,
        ),
        decision(
            candidate,
            DecisionRole::Risk,
            3,
            12,
            DecisionValue::Approve,
            10_011,
            10_800,
        ),
        decision(
            candidate,
            DecisionRole::Operations,
            4,
            13,
            DecisionValue::Approve,
            10_012,
            10_800,
        ),
        ReadinessCommand::Finalize {
            command_id: ReadinessCommandId(id(5)),
            candidate_id: candidate.candidate_id,
            record_id: id(220),
            evaluated_at_ns: 10_100,
            recorded_at_ns: 10_100,
        },
    ]
}

fn run(commands: &[ReadinessCommand]) -> ProductionChangeReadiness {
    let mut owner = ProductionChangeReadiness::new(policy()).expect("owner");
    for command in commands {
        owner.apply(command).expect("command");
    }
    owner
}

#[test]
fn fresh_diverse_regression_safe_three_role_evidence_is_ready() {
    let record = run(&commands_for(&candidate()))
        .snapshot()
        .last_record
        .expect("record");
    assert_eq!(record.status, ReadinessStatus::ProductionChangeReady);
    assert!(record.reasons.is_empty());
    assert_eq!(record.eligible_campaign_count, 2);
    assert_eq!(record.case_count, 8);
    assert_eq!(record.independent_plan_count, 8);
    assert_eq!(record.restart_count, 2);
    assert_eq!(record.approval_set_count, 8);
    assert_eq!(record.regression_case_floor, 8);
    assert!(record.operator_execution_required);
    assert!(!record.credential_material_created);
    assert!(!record.authentication_authority_granted);
    assert!(!record.deployment_authority_granted);
    assert!(!record.rollback_execution_authority_granted);
    assert!(!record.traffic_authority_granted);
    assert!(!record.cloud_control_authority_granted);
    assert!(!record.live_trading_authority_granted);
}

#[test]
fn subject_substitution_and_authority_bearing_evidence_halt_registration() {
    let mut substituted = candidate();
    substituted.subject.configuration_digest = id(199);
    substituted = substituted.sealed(&policy());
    let mut owner = ProductionChangeReadiness::new(policy()).expect("owner");
    assert!(matches!(
        owner.apply(&ReadinessCommand::Register {
            command_id: ReadinessCommandId(id(1)),
            recorded_at_ns: substituted.created_at_ns,
            candidate: Box::new(substituted),
        }),
        Err(Error::Candidate)
    ));

    let mut unsafe_evidence = evidence(1, 9_900);
    unsafe_evidence.deployment_authority_granted = true;
    unsafe_evidence.evidence_digest = campaign_evidence_digest(&unsafe_evidence);
    let candidate = candidate_with(vec![unsafe_evidence, evidence(40, 9_901)]);
    let mut owner = ProductionChangeReadiness::new(policy()).expect("owner");
    assert!(matches!(
        owner.apply(&ReadinessCommand::Register {
            command_id: ReadinessCommandId(id(2)),
            recorded_at_ns: candidate.created_at_ns,
            candidate: Box::new(candidate),
        }),
        Err(Error::Evidence)
    ));
}

#[test]
fn duplicate_stale_and_ineligible_evidence_are_attributable() {
    let first = evidence(1, 7_000);
    let duplicate = first.clone();
    let mut ineligible = evidence(40, 9_901);
    ineligible.status = CampaignEvidenceStatus::NotEligible;
    ineligible.reasons = vec![CampaignEvidenceReason::CasesIncomplete];
    ineligible.evidence_digest = campaign_evidence_digest(&ineligible);
    let candidate = candidate_with(vec![first, duplicate, ineligible]);
    let record = run(&commands_for(&candidate))
        .snapshot()
        .last_record
        .expect("record");
    assert_eq!(record.status, ReadinessStatus::NotReady);
    assert!(record.reasons.contains(&ReadinessReason::DuplicateEvidence));
    assert!(record.reasons.contains(&ReadinessReason::DuplicateCampaign));
    assert!(record.reasons.contains(&ReadinessReason::EvidenceStale));
    assert!(record
        .reasons
        .contains(&ReadinessReason::EvidenceIneligible));
}

#[test]
fn missing_rejected_expired_and_same_operator_decisions_deny_readiness() {
    let candidate = candidate();
    let mut commands = commands_for(&candidate);
    commands.remove(3);
    let record = run(&commands).snapshot().last_record.expect("record");
    assert!(record
        .reasons
        .contains(&ReadinessReason::DecisionMissing(DecisionRole::Operations)));

    let mut commands = commands_for(&candidate);
    commands[2] = decision(
        &candidate,
        DecisionRole::Risk,
        3,
        12,
        DecisionValue::Reject,
        10_011,
        10_800,
    );
    let record = run(&commands).snapshot().last_record.expect("record");
    assert!(record
        .reasons
        .contains(&ReadinessReason::DecisionRejected(DecisionRole::Risk)));

    let mut commands = commands_for(&candidate);
    commands[1] = decision(
        &candidate,
        DecisionRole::Release,
        2,
        11,
        DecisionValue::Approve,
        10_010,
        10_050,
    );
    let record = run(&commands).snapshot().last_record.expect("record");
    assert!(record
        .reasons
        .contains(&ReadinessReason::DecisionExpired(DecisionRole::Release)));

    let mut commands = commands_for(&candidate);
    commands[2] = decision(
        &candidate,
        DecisionRole::Risk,
        3,
        11,
        DecisionValue::Approve,
        10_011,
        10_800,
    );
    let record = run(&commands).snapshot().last_record.expect("record");
    assert!(record
        .reasons
        .contains(&ReadinessReason::OperatorsNotDistinct));
}

#[test]
fn stricter_regression_and_diversity_floors_deny_without_overflow() {
    let mut strict = policy();
    strict.minimum_plan_diversity = 9;
    strict.minimum_case_count = 9;
    let candidate = ProductionReadinessCandidate {
        policy_digest: [0; 32],
        candidate_digest: [0; 32],
        ..candidate()
    }
    .sealed(&strict);
    let mut owner = ProductionChangeReadiness::new(strict).expect("owner");
    let mut commands = commands_for(&candidate);
    if let ReadinessCommand::Register {
        candidate: slot, ..
    } = &mut commands[0]
    {
        *slot = Box::new(candidate);
    }
    for command in commands {
        owner.apply(&command).expect("command");
    }
    let record = owner.snapshot().last_record.expect("record");
    assert!(record.reasons.contains(&ReadinessReason::PlanDiversity));
    assert!(record.reasons.contains(&ReadinessReason::CaseRegression));
}

#[test]
fn record_file_is_create_new_and_corruption_detecting() {
    let record = run(&commands_for(&candidate()))
        .snapshot()
        .last_record
        .expect("record");
    let directory = tempdir().expect("directory");
    let path = directory.path().join("readiness.record");
    write_record_create_new(&path, &record).expect("write");
    assert_eq!(read_record(&path).expect("read"), record);
    assert!(matches!(
        write_record_create_new(&path, &record),
        Err(ProductionReadinessRecordFileError::Io(_))
    ));
    let mut bytes = std::fs::read(&path).expect("bytes");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, bytes).expect("corrupt");
    assert!(matches!(
        read_record(&path),
        Err(ProductionReadinessRecordFileError::Checksum)
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
    let commands = commands_for(&candidate());
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
    let recovery = ReadinessRecovery {
        readiness: ProductionChangeReadiness::new(policy()).expect("owner"),
        last_sequence: None,
    };
    let mut durable = DurableProductionReadiness::new(writer, recovery).expect("durable");
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let expected = durable.readiness().snapshot().digest;
    let checkpoint = ReadinessCheckpoint {
        sequence: u64::try_from(commands.len() - 1).expect("sequence"),
        readiness_digest: expected,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.readiness.snapshot().digest, expected);

    let recovery = ReadinessRecovery {
        readiness: ProductionChangeReadiness::new(policy()).expect("owner"),
        last_sequence: None,
    };
    let mut failing =
        DurableProductionReadiness::new(FailingJournal::default(), recovery).expect("owner");
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(ReadinessStorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(ReadinessStorageError::Halted(_))
    ));
    assert_eq!(failing.readiness().snapshot().accepted_commands, 0);
}

#[test]
fn identical_commands_produce_identical_complete_state() {
    let commands = commands_for(&candidate());
    assert_eq!(run(&commands).snapshot(), run(&commands).snapshot());
}

proptest! {
    #[test]
    fn retained_floor_is_monotonic(baseline in 1_usize..1_000, low in 1_u16..10_000, high in 1_u16..10_001) {
        prop_assume!(low <= high);
        let low_floor = retained_floor(baseline, low).expect("low");
        let high_floor = retained_floor(baseline, high).expect("high");
        prop_assert!(low_floor <= high_floor);
    }
}
